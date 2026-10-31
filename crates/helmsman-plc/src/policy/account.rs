//! Local accounts: one login per person, so the audit log names who
//! approved a change, not just which role's PIN was typed.
//!
//! **File** (`accounts` in the state directory, mode 0600, refused if
//! anyone else may read it), one account per line, hand-parsed:
//!
//! ```text
//! helmsman-accounts 1
//! alice engineer enabled pbkdf2-sha256 600000 <salt, 32 hex> <hash, 64 hex>
//! bob   operator disabled pbkdf2-sha256 600000 <salt> <hash>
//! ```
//!
//! The password hash is the PIN file's (PBKDF2-HMAC-SHA256 over the
//! `sha2` the box already carries, [`super::pin::pbkdf2`]), with the same
//! iteration floor ([`super::pin::MIN_ITERATIONS`]). `helmsman-plc account
//! add|passwd|disable|enable|list` edits it ([`cli`]); a password is only
//! ever read from stdin.
//!
//! **Accounts replace PINs.** While the file exists the box takes a
//! session from `POST /api/login` for apply and rollback and refuses any
//! role PIN, even a right one; without it the PINs work as before. The
//! file is read again on every login and every use of a session, so
//! disabling an account, changing its password or its role, or removing
//! it ends its sessions at their next use, without a restart.
//!
//! **Logins** ([`Logins`]): a wrong password counts toward a lockout of
//! that account (the policy's [`Lockout`], doubling per further wrong
//! password up to [`MAX_LOCKOUT`]) and toward a lockout of every login on
//! the box ([`GLOBAL_FACTOR`] times the allowance within
//! [`GLOBAL_WINDOW`]), so spreading guesses over accounts gains little.
//! Checks are serialized (one PBKDF2 at a time). An unknown user costs the
//! same PBKDF2 as a known one, and every comparison is constant-time.
//!
//! **Sessions**: 32 random bytes as hex, held server-side only as their
//! SHA-256, ending after [`SESSION_TTL`], after [`SESSION_IDLE`] unused,
//! on logout, or when the account changes. A session carries its
//! account's role; nothing a request says can change it.
//!
//! **Role-PIN sessions** ([`Logins::issue_role`]): on a box without an
//! accounts file, a role's PIN (checked by the caller through the PIN
//! gate and its lockout) opens a session for that role, in the same
//! table under the same limits, with no person ([`Proof::Pin`]); it ends
//! at its next use once an accounts file appears.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt::{self, Write as _};
use std::fs;
use std::io::{self, BufRead, Read as _};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use super::pin::{self, Clock, Record, ct_eq, hex, read_private, write_private};
use super::{Lockout, MAX_LOCKOUT, Role};

/// The accounts file's name in the state directory.
pub const FILE: &str = "accounts";
/// The accounts file's first line.
pub const HEADER: &str = "helmsman-accounts 1";
/// Largest accounts file, in bytes.
pub const MAX_FILE: usize = 256 * 1024;
/// Most accounts a file may hold.
pub const MAX_ACCOUNTS: usize = 1000;
/// Longest user name: `[a-z0-9][a-z0-9._-]*`.
pub const USER_LEN: usize = 32;
/// Shortest and longest password, in characters (no control characters).
pub const PASSWORD_LEN: (usize, usize) = (8, 128);
/// A login password longer than this, in bytes, is wrong without being
/// hashed as given (PBKDF2 rehashes a long key every iteration).
pub const MAX_PASSWORD_BYTES: usize = 1024;
/// A session ends this long after its login, however busy.
pub const SESSION_TTL: Duration = Duration::from_secs(60 * 60);
/// A session ends after this long unused.
pub const SESSION_IDLE: Duration = Duration::from_secs(15 * 60);
/// Most sessions the box holds at once.
pub const MAX_SESSIONS: usize = 256;
/// Most sessions one account holds; a further login ends its oldest.
pub const SESSIONS_PER_USER: usize = 8;
/// Wrong passwords across every account, as a multiple of the per-account
/// allowance, before all logins lock.
pub const GLOBAL_FACTOR: u32 = 4;
/// How long a wrong password counts toward the box-wide lockout.
pub const GLOBAL_WINDOW: Duration = Duration::from_secs(60 * 60);

/// Whether `u` may name an account: 1..=[`USER_LEN`] of `a-z 0-9 . _ -`,
/// starting with a letter or digit.
#[must_use]
pub fn valid_user(u: &str) -> bool {
    let b = u.as_bytes();
    (1..=USER_LEN).contains(&b.len())
        && b[0].is_ascii_alphanumeric()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"._-".contains(c))
}

/// Whether `p` may be set as a password: [`PASSWORD_LEN`] characters, no
/// control characters.
#[must_use]
pub fn acceptable_password(p: &str) -> bool {
    (PASSWORD_LEN.0..=PASSWORD_LEN.1).contains(&p.chars().count()) && !p.chars().any(char::is_control)
}

/// One person.
#[derive(Clone, PartialEq, Eq)]
pub struct Account {
    pub user: String,
    pub role: Role,
    /// A disabled account cannot log in, and its sessions end.
    pub disabled: bool,
    secret: Record,
}

impl fmt::Debug for Account {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Account")
            .field("user", &self.user)
            .field("role", &self.role)
            .field("disabled", &self.disabled)
            .finish_non_exhaustive()
    }
}

impl Account {
    fn line(&self) -> String {
        format!(
            "{} {} {} {}",
            self.user,
            self.role,
            if self.disabled { "disabled" } else { "enabled" },
            self.secret.render()
        )
    }

    /// Changes with anything about the account: a session made under one
    /// stamp ends when the stamp changes.
    fn stamp(&self) -> [u8; 32] {
        Sha256::digest(self.line().as_bytes()).into()
    }

    /// The iterations its hash takes.
    #[must_use]
    pub const fn iterations(&self) -> u32 {
        self.secret.iterations
    }
}

/// Every account. `Debug` shows names, roles and states, never hashes.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Accounts {
    list: BTreeMap<String, Account>,
}

impl fmt::Debug for Accounts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.list.values()).finish()
    }
}

fn password_rule() -> String {
    format!(
        "a password is {}..={} characters, no control characters",
        PASSWORD_LEN.0, PASSWORD_LEN.1
    )
}

impl Accounts {
    /// Adds `user` with `role`, enabled.
    ///
    /// # Errors
    /// The name is not [`valid_user`] or is taken, the password is not
    /// [`acceptable_password`], `iterations` is zero, or the file is full.
    pub fn add(
        &mut self,
        user: &str,
        role: Role,
        password: &str,
        iterations: u32,
        salt: [u8; 16],
    ) -> Result<(), String> {
        if !valid_user(user) {
            return Err(format!(
                "a user name is 1..={USER_LEN} of a-z, 0-9, '.', '_', '-', starting with a letter or digit"
            ));
        }
        if self.list.contains_key(user) {
            return Err(format!("{user} already has an account"));
        }
        if self.list.len() >= MAX_ACCOUNTS {
            return Err(format!("at most {MAX_ACCOUNTS} accounts"));
        }
        if !acceptable_password(password) {
            return Err(password_rule());
        }
        if iterations == 0 {
            return Err("iterations must be at least 1".into());
        }
        self.list.insert(
            user.to_string(),
            Account {
                user: user.to_string(),
                role,
                disabled: false,
                secret: Record::new(password, iterations, salt),
            },
        );
        Ok(())
    }

    /// Sets `user`'s password (its sessions end).
    ///
    /// # Errors
    /// No such account, or the password is not [`acceptable_password`].
    pub fn passwd(
        &mut self,
        user: &str,
        password: &str,
        iterations: u32,
        salt: [u8; 16],
    ) -> Result<(), String> {
        if !acceptable_password(password) {
            return Err(password_rule());
        }
        if iterations == 0 {
            return Err("iterations must be at least 1".into());
        }
        let a = self
            .list
            .get_mut(user)
            .ok_or_else(|| format!("no account {user:?}"))?;
        a.secret = Record::new(password, iterations, salt);
        Ok(())
    }

    /// Disables or enables `user` (disabling ends its sessions).
    ///
    /// # Errors
    /// No such account.
    pub fn set_disabled(&mut self, user: &str, disabled: bool) -> Result<(), String> {
        let a = self
            .list
            .get_mut(user)
            .ok_or_else(|| format!("no account {user:?}"))?;
        a.disabled = disabled;
        Ok(())
    }

    #[must_use]
    pub fn get(&self, user: &str) -> Option<&Account> {
        self.list.get(user)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Account> {
        self.list.values()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.list.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// The file [`Accounts::parse`] reads.
    #[must_use]
    pub fn render(&self) -> String {
        let mut s = format!("{HEADER}\n");
        for a in self.list.values() {
            s.push_str(&a.line());
            s.push('\n');
        }
        s
    }

    /// An accounts file; every hash must use at least `min_iterations`.
    ///
    /// # Errors
    /// Every bad line, by number (0: the file as a whole).
    pub fn parse(src: &str, min_iterations: u32) -> Result<Self, Vec<(usize, String)>> {
        if src.len() > MAX_FILE {
            return Err(vec![(0, format!("larger than {MAX_FILE} bytes"))]);
        }
        let mut accounts = Self::default();
        let mut errs = Vec::new();
        let mut header = false;
        for (n, raw) in src.lines().enumerate() {
            let n = n + 1;
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if !raw.bytes().all(|b| b == b'\t' || (b' '..=b'~').contains(&b)) {
                errs.push((n, "only printable ASCII".into()));
                continue;
            }
            if !header {
                header = true;
                if line != HEADER {
                    errs.push((n, format!("expected {HEADER:?} first")));
                }
                continue;
            }
            let w: Vec<&str> = line.split_whitespace().collect();
            let parsed = match w.as_slice() {
                [user, role, state, rest @ ..] if valid_user(user) => Role::parse(role)
                    .zip(match *state {
                        "enabled" => Some(false),
                        "disabled" => Some(true),
                        _ => None,
                    })
                    .zip(Record::parse(rest))
                    .map(|((role, disabled), secret)| Account {
                        user: (*user).to_string(),
                        role,
                        disabled,
                        secret,
                    }),
                _ => None,
            };
            match parsed {
                None => errs.push((
                    n,
                    "expected: USER ROLE enabled|disabled pbkdf2-sha256 ITERATIONS SALT HASH".into(),
                )),
                Some(a) if a.secret.iterations < min_iterations => errs.push((
                    n,
                    format!("{} iterations, fewer than {min_iterations}", a.secret.iterations),
                )),
                Some(a) if accounts.list.contains_key(&a.user) => {
                    errs.push((n, format!("{} given twice", a.user)));
                }
                Some(_) if accounts.list.len() >= MAX_ACCOUNTS => {
                    errs.push((n, format!("more than {MAX_ACCOUNTS} accounts")));
                }
                Some(a) => {
                    accounts.list.insert(a.user.clone(), a);
                }
            }
        }
        if !header {
            errs.push((0, format!("empty; expected {HEADER:?}")));
        }
        if errs.is_empty() { Ok(accounts) } else { Err(errs) }
    }

    /// Reads `dir/accounts`: `None` when there is no such file (the box
    /// uses role PINs).
    ///
    /// # Errors
    /// The file cannot be read, is not private, or does not parse.
    pub fn load(dir: &Path, min_iterations: u32) -> Result<Option<Self>, String> {
        let path = dir.join(FILE);
        let Some(src) = read_private(&path)? else {
            return Ok(None);
        };
        Self::parse(&src, min_iterations).map(Some).map_err(|errs| {
            let lines: Vec<String> = errs
                .iter()
                .map(|(n, e)| format!("\n  {}:{n}: {e}", path.display()))
                .collect();
            format!("{} does not parse:{}", path.display(), lines.concat())
        })
    }

    /// Writes `dir/accounts`, private, whole.
    ///
    /// # Errors
    /// The directory or file cannot be written.
    pub fn save(&self, dir: &Path) -> io::Result<()> {
        write_private(dir, FILE, &self.render())
    }
}

/// Who a session is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Principal {
    /// The account; for a role-PIN session, the role's name (no person
    /// is known: see [`Principal::person`]).
    pub user: String,
    pub role: Role,
    /// What the login proved.
    pub proof: Proof,
}

/// What a session's login checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Proof {
    /// An account's password (a box with an accounts file).
    Password,
    /// A role's PIN (a box without one): the session is the role's, no
    /// person's.
    Pin,
}

impl Principal {
    /// The person, when the session is an account's.
    #[must_use]
    pub fn person(&self) -> Option<&str> {
        match self.proof {
            Proof::Password => Some(&self.user),
            Proof::Pin => None,
        }
    }

    /// How the audit log and the history name the approver: `alice
    /// (engineer)`, or `operator (PIN)` for a role-PIN session.
    #[must_use]
    pub fn label(&self) -> String {
        match self.proof {
            Proof::Password => format!("{} ({})", self.user, self.role),
            Proof::Pin => format!("{} (PIN)", self.role),
        }
    }
}

/// What a role-PIN session is checked against at each use: it lasts
/// while the box has no accounts file (one appearing ends it at its next
/// use). PINs change only with a restart, which ends every session.
fn pin_stamp(role: Role) -> [u8; 32] {
    Sha256::digest(format!("helmsman role-pin session {role}").as_bytes()).into()
}

/// A login that worked. `Debug` never shows the token.
#[derive(Clone, PartialEq, Eq)]
pub struct Issued {
    /// The session token, 64 hex digits: `Authorization: Bearer` it.
    pub token: String,
    pub who: Principal,
    pub ttl: Duration,
    pub idle: Duration,
}

impl fmt::Debug for Issued {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Issued")
            .field("who", &self.who)
            .field("ttl", &self.ttl)
            .field("idle", &self.idle)
            .finish_non_exhaustive()
    }
}

/// Why a login was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoginRefused {
    /// Logins (for this account, or for all) are locked; nothing was
    /// checked. Try again after this long.
    Locked(Duration),
    /// Wrong user or password, or a disabled account (not told apart).
    Wrong,
    /// The box has no accounts file: it uses role PINs.
    NoAccounts,
    /// The accounts file cannot be used (why); nobody can log in.
    Unavailable(String),
    /// The session table is full.
    Full,
}

/// Why a session was not taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionRefused {
    /// No such session (never issued, logged out, or evicted).
    Unknown,
    /// Past [`SessionLimits::ttl`] or [`SessionLimits::idle`].
    Expired,
    /// Its account was disabled, removed or changed since the login.
    Revoked,
}

/// What credentials the box takes for apply and rollback, read from the
/// state directory.
#[derive(Debug)]
pub enum Mode {
    /// No accounts file: role PINs.
    Pins,
    /// Accounts: sessions only, PINs refused.
    Accounts(Accounts),
    /// An accounts file that cannot be used: nothing is taken.
    Broken(String),
}

/// How long sessions last and how many there may be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionLimits {
    pub ttl: Duration,
    pub idle: Duration,
    pub max: usize,
    pub per_user: usize,
}

impl Default for SessionLimits {
    fn default() -> Self {
        Self {
            ttl: SESSION_TTL,
            idle: SESSION_IDLE,
            max: MAX_SESSIONS,
            per_user: SESSIONS_PER_USER,
        }
    }
}

#[derive(Default)]
struct Fails {
    failures: u32,
    locked_until: Option<Instant>,
}

#[derive(Default)]
struct LoginState {
    /// Per existing account: wrong passwords since its last login.
    users: HashMap<String, Fails>,
    /// When each recent wrong password (any account) came.
    global: VecDeque<Instant>,
    global_until: Option<Instant>,
}

struct Session {
    who: Principal,
    stamp: [u8; 32],
    created: Instant,
    last: Instant,
}

/// Logins and sessions for one box.
pub struct Logins {
    dir: PathBuf,
    min_iterations: u32,
    lockout: Lockout,
    clock: Clock,
    limits: SessionLimits,
    state: Mutex<LoginState>,
    /// By the SHA-256 of the token: the token itself is never kept.
    sessions: Mutex<HashMap<[u8; 32], Session>>,
}

impl fmt::Debug for Logins {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Logins")
            .field("dir", &self.dir)
            .field("lockout", &self.lockout)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn key(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

/// The lockout after `over` wrong passwords past the allowance.
fn backoff(lockout: Lockout, over: u32) -> Duration {
    let factor = 1u32.checked_shl(over.min(31)).unwrap_or(u32::MAX);
    lockout.first.saturating_mul(factor).min(MAX_LOCKOUT)
}

impl Logins {
    /// Logins against `dir/accounts`, hashes of at least `min_iterations`,
    /// locking out per `lockout`.
    #[must_use]
    pub fn new(dir: PathBuf, min_iterations: u32, lockout: Lockout, clock: Clock) -> Self {
        Self {
            dir,
            min_iterations,
            lockout,
            clock,
            limits: SessionLimits::default(),
            state: Mutex::new(LoginState::default()),
            sessions: Mutex::new(HashMap::new()),
        }
    }

    #[must_use]
    pub const fn with_limits(mut self, limits: SessionLimits) -> Self {
        self.limits = limits;
        self
    }

    #[must_use]
    pub const fn limits(&self) -> SessionLimits {
        self.limits
    }

    /// What the box takes now, from the accounts file as it is on disk.
    #[must_use]
    pub fn mode(&self) -> Mode {
        match Accounts::load(&self.dir, self.min_iterations) {
            Ok(None) => Mode::Pins,
            Ok(Some(a)) => Mode::Accounts(a),
            Err(e) => Mode::Broken(e),
        }
    }

    /// Checks `user`'s `password` and, if right, starts a session.
    ///
    /// # Errors
    /// [`LoginRefused`].
    pub fn login(&self, user: &str, password: &str) -> Result<Issued, LoginRefused> {
        let mut st = lock(&self.state);
        let now = (self.clock)();
        if let Some(until) = st.global_until {
            if now < until {
                return Err(LoginRefused::Locked(until - now));
            }
            st.global_until = None;
        }
        let accounts = match self.mode() {
            Mode::Pins => return Err(LoginRefused::NoAccounts),
            Mode::Broken(e) => return Err(LoginRefused::Unavailable(e)),
            Mode::Accounts(a) => a,
        };
        let account = accounts.get(user);
        if account.is_some()
            && let Some(f) = st.users.get_mut(user)
            && let Some(until) = f.locked_until
        {
            if now < until {
                return Err(LoginRefused::Locked(until - now));
            }
            f.locked_until = None;
        }
        // Over-long passwords are wrong, but cost the same PBKDF2.
        let long = password.len() > MAX_PASSWORD_BYTES;
        let typed = if long { "" } else { password };
        let ok = if let Some(a) = account {
            a.secret.matches(typed) && !long && !a.disabled
        } else {
            // An unknown user takes as long as a known one.
            let iterations = accounts
                .iter()
                .next()
                .map_or(pin::ITERATIONS, Account::iterations);
            let dummy = Record {
                iterations,
                salt: [0; 16],
                hash: [0; 32],
            };
            let _ = dummy.matches(typed);
            false
        };
        if let (true, Some(a)) = (ok, account) {
            st.users.remove(user);
            drop(st);
            let who = Principal {
                user: a.user.clone(),
                role: a.role,
                proof: Proof::Password,
            };
            return self.issue(who, a.stamp(), now);
        }
        if account.is_some() {
            let f = st.users.entry(user.to_string()).or_default();
            f.failures = f.failures.saturating_add(1);
            if let Some(over) = f.failures.checked_sub(self.lockout.attempts) {
                f.locked_until = Some(now + backoff(self.lockout, over));
            }
        }
        while st
            .global
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) >= GLOBAL_WINDOW)
        {
            st.global.pop_front();
        }
        let allowance = self.lockout.attempts.saturating_mul(GLOBAL_FACTOR);
        st.global.push_back(now);
        // Past 32 more the lockout is already at its longest.
        let cap = usize::try_from(allowance)
            .unwrap_or(usize::MAX)
            .saturating_add(32);
        while st.global.len() > cap {
            st.global.pop_front();
        }
        let count = u32::try_from(st.global.len()).unwrap_or(u32::MAX);
        if let Some(over) = count.checked_sub(allowance) {
            st.global_until = Some(now + backoff(self.lockout, over));
        }
        Err(LoginRefused::Wrong)
    }

    /// Opens a session for `role` once its PIN was checked: by the
    /// caller, through [`super::pin::Gate::check`] and its lockout; this
    /// checks no PIN. It lasts as an account's session does, counts toward
    /// the same caps (the per-person one per role), and ends at its next
    /// use once the box has an accounts file.
    ///
    /// # Errors
    /// [`LoginRefused::Unavailable`] if the box has an accounts file (role
    /// PINs are off) or no randomness, [`LoginRefused::Full`] if the
    /// table is.
    pub fn issue_role(&self, role: Role) -> Result<Issued, LoginRefused> {
        if !matches!(self.mode(), Mode::Pins) {
            return Err(LoginRefused::Unavailable(
                "this box has accounts: role PINs are off".into(),
            ));
        }
        let who = Principal {
            user: role.as_str().to_string(),
            role,
            proof: Proof::Pin,
        };
        self.issue(who, pin_stamp(role), (self.clock)())
    }

    fn issue(&self, who: Principal, stamp: [u8; 32], now: Instant) -> Result<Issued, LoginRefused> {
        let token = random_token().map_err(|e| LoginRefused::Unavailable(format!("random token: {e}")))?;
        let mut ss = lock(&self.sessions);
        let limits = self.limits;
        ss.retain(|_, s| {
            now.saturating_duration_since(s.created) < limits.ttl
                && now.saturating_duration_since(s.last) < limits.idle
        });
        let mut mine: Vec<([u8; 32], Instant)> = ss
            .iter()
            .filter(|(_, s)| s.who.user == who.user && s.who.proof == who.proof)
            .map(|(k, s)| (*k, s.created))
            .collect();
        mine.sort_by_key(|(_, c)| *c);
        let excess = (mine.len() + 1).saturating_sub(limits.per_user.max(1));
        for (k, _) in mine.into_iter().take(excess) {
            ss.remove(&k);
        }
        if ss.len() >= limits.max {
            return Err(LoginRefused::Full);
        }
        ss.insert(
            key(&token),
            Session {
                who: who.clone(),
                stamp,
                created: now,
                last: now,
            },
        );
        Ok(Issued {
            token,
            who,
            ttl: limits.ttl,
            idle: limits.idle,
        })
    }

    /// Who `token`'s session is, if it is live and its account unchanged;
    /// using it restarts its idle time.
    ///
    /// # Errors
    /// [`SessionRefused`]; a session refused for any reason but
    /// [`SessionRefused::Unknown`] is ended.
    pub fn session(&self, token: &str) -> Result<Principal, SessionRefused> {
        let k = key(token);
        let now = (self.clock)();
        let mut ss = lock(&self.sessions);
        let Some(s) = ss.get_mut(&k) else {
            return Err(SessionRefused::Unknown);
        };
        if now.saturating_duration_since(s.created) >= self.limits.ttl
            || now.saturating_duration_since(s.last) >= self.limits.idle
        {
            ss.remove(&k);
            return Err(SessionRefused::Expired);
        }
        let current = match (self.mode(), s.who.proof) {
            (Mode::Accounts(a), Proof::Password) => {
                a.get(&s.who.user).filter(|a| !a.disabled).map(Account::stamp)
            }
            (Mode::Pins, Proof::Pin) => Some(pin_stamp(s.who.role)),
            _ => None,
        };
        if !current.is_some_and(|c| ct_eq(&c, &s.stamp)) {
            ss.remove(&k);
            return Err(SessionRefused::Revoked);
        }
        s.last = now;
        Ok(s.who.clone())
    }

    /// Ends `token`'s session; who it was, if it was one.
    pub fn logout(&self, token: &str) -> Option<Principal> {
        lock(&self.sessions).remove(&key(token)).map(|s| s.who)
    }

    /// Sessions held now (live or not yet swept).
    #[must_use]
    pub fn sessions(&self) -> usize {
        lock(&self.sessions).len()
    }
}

fn random_token() -> io::Result<String> {
    let mut b = [0u8; 32];
    fs::File::open("/dev/urandom")?.read_exact(&mut b)?;
    Ok(hex(&b))
}

/// `helmsman-plc account ...`: edits the accounts file in the state
/// directory (`--state-dir DIR`, else `default_dir`); passwords come from
/// `stdin`, one line each, never from `args`. What to print on success.
///
/// # Errors
/// A usage or file problem, said plainly.
pub fn cli(args: &[String], stdin: &mut dyn BufRead, default_dir: PathBuf) -> Result<String, String> {
    let mut dir = default_dir;
    let mut words: Vec<&str> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--state-dir" {
            dir = PathBuf::from(it.next().ok_or("--state-dir needs a value")?);
        } else {
            words.push(a);
        }
    }
    let path = dir.join(FILE);
    let existed = path.exists();
    let mut accounts = Accounts::load(&dir, pin::MIN_ITERATIONS)?.unwrap_or_default();
    let mut password = |user: &str| -> Result<String, String> {
        eprintln!("helmsman-plc: password for {user} (one line on stdin):");
        let mut line = String::new();
        stdin.read_line(&mut line).map_err(|e| format!("stdin: {e}"))?;
        Ok(line.trim_end_matches(['\r', '\n']).to_string())
    };
    let salt = || pin::salt().map_err(|e| format!("random salt: {e}"));
    let done = match words.as_slice() {
        ["list"] => {
            if !existed {
                return Ok(format!(
                    "no accounts file in {}: approvals use role PINs\n",
                    dir.display()
                ));
            }
            let mut out = String::new();
            for a in accounts.iter() {
                let _ = writeln!(
                    out,
                    "{} {} {}",
                    a.user,
                    a.role,
                    if a.disabled { "disabled" } else { "enabled" }
                );
            }
            return Ok(out);
        }
        ["add", user, role] => {
            let role = Role::parse(role)
                .ok_or_else(|| format!("{role:?}: expected operator, engineer or integrator"))?;
            if accounts.get(user).is_some() {
                return Err(format!("{user} already has an account"));
            }
            let p = password(user)?;
            accounts.add(user, role, &p, pin::ITERATIONS, salt()?)?;
            format!("{user} ({role}) added")
        }
        ["passwd", user] => {
            if accounts.get(user).is_none() {
                return Err(format!("no account {user:?}"));
            }
            let p = password(user)?;
            accounts.passwd(user, &p, pin::ITERATIONS, salt()?)?;
            format!("{user}'s password changed; its sessions end")
        }
        ["disable", user] => {
            accounts.set_disabled(user, true)?;
            format!("{user} disabled; its sessions end")
        }
        ["enable", user] => {
            accounts.set_disabled(user, false)?;
            format!("{user} enabled")
        }
        _ => {
            return Err(
                "usage: helmsman-plc account add USER ROLE | passwd USER | disable USER | enable USER | list [--state-dir DIR]\n\
                 (passwords are read from stdin, one line)"
                    .into(),
            );
        }
    };
    accounts
        .save(&dir)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let mut out = format!("{done} ({}); in effect at the next request\n", path.display());
    if !existed {
        out.push_str(
            "this box now has accounts: role PINs are refused; approvers log in (POST /api/login)\n",
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_names() {
        for good in ["a", "alice", "j.smith", "op_2", "x-1", &"a".repeat(USER_LEN)] {
            assert!(valid_user(good), "{good:?}");
        }
        for bad in [
            "",
            "Alice",
            ".a",
            "-a",
            "a b",
            "a/b",
            "é",
            &"a".repeat(USER_LEN + 1),
        ] {
            assert!(!valid_user(bad), "{bad:?}");
        }
    }

    #[test]
    fn ct_eq_is_equality() {
        let a = [7u8; 32];
        assert!(ct_eq(&a, &a));
        for k in 0..32 {
            let mut b = a;
            b[k] ^= 1;
            assert!(!ct_eq(&a, &b), "differs at {k}");
        }
        assert!(!ct_eq(&a, &a[..31]));
    }
}
