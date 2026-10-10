//! Accounts and the signed-in session.
//!
//! The pure logic (the database, password hashing, permission rules, login throttling) is in
//! `kitsune_core::{account, password, perm, session}`; this file is the glue: where the database
//! lives (`/etc/accounts`, written with full rights), where the salt comes from (the system
//! entropy pool), the clock, and the one global *session* that `vfs` reads to decide whose
//! permissions apply.
//!
//! First boot after accounts exist: there is no `/etc/accounts`, so one is created with a single
//! administrator **without a password** (`kitsune`) who adopts everything already on the volume
//! (see `kitsune_core::homes`). A lone account with no password signs in by itself, so the
//! desktop opens as it always did; setting a password (Ajustes > Usuários) or adding a second
//! account turns the login screen on.

use super::vfs;
use crate::sync::YieldMutex;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use kitsune_core::account::{AccountDb, AccountError, AuthError, User};
use kitsune_core::homes as layout;
use kitsune_core::password::{self, SALT_LEN};
use kitsune_core::perm::Cred;
use kitsune_core::session::LoginGuard;

/// Where the database lives.
const PATH: &[u8] = b"/etc/accounts";

/// The account a fresh volume gets.
const DEFAULT_USER: &str = "kitsune";

static DB: YieldMutex<Option<AccountDb>> = YieldMutex::new(None);
static SESSION: YieldMutex<Option<Session>> = YieldMutex::new(None);
static GUARD: YieldMutex<Option<LoginGuard>> = YieldMutex::new(None);

/// Who is signed in.
#[derive(Clone, Debug)]
pub struct Session {
    pub name: String,
    pub full_name: String,
    pub uid: u32,
    pub home: String,
    pub admin: bool,
    pub cred: Cred,
}

/// One row of the user list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserRow {
    pub name: String,
    pub full_name: String,
    pub uid: u32,
    pub admin: bool,
    pub locked: bool,
    pub has_password: bool,
}

/// Why a sign-in failed. The first two look the same to the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginFail {
    Unknown,
    BadPassword,
    Locked,
    /// Too many wrong passwords: try again in this many milliseconds.
    Throttled(u64),
}

/// Why an account change was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChangeFail {
    /// Not an administrator (and not changing one's own password).
    Denied,
    /// The current password given is wrong.
    BadPassword,
    Password(password::PasswordError),
    Account(AccountError),
    /// The disk refused the write.
    Storage,
}

fn now_ms() -> u64 {
    crate::netd::now_ms()
}

fn now() -> u64 {
    crate::storage::now()
}

fn snapshot(db: &AccountDb, u: &User) -> Session {
    Session {
        name: u.name.clone(),
        full_name: u.full_name.clone(),
        uid: u.uid,
        home: u.home.clone(),
        admin: db.is_admin(u),
        cred: db.cred(u),
    }
}

fn save(db: &AccountDb) -> bool {
    vfs::root::write_file(PATH, db.serialize().as_bytes()).is_ok()
}

fn with_db<R>(f: impl FnOnce(&mut AccountDb) -> R) -> Option<R> {
    let mut g = DB.lock().ok()?;
    g.as_mut().map(f)
}

/// Load the database (creating it on a fresh volume) and sign in the lone no-password account.
/// Call once, after the storage is up.
pub fn init() {
    let loaded = vfs::root::read_file(PATH)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|t| AccountDb::parse(&t).ok());
    let db = match loaded {
        Some(db) => {
            crate::serial_println!("accounts: {} users", db.users().len());
            // A volume copied from elsewhere may lack the folders; keep them right.
            let _ = vfs::root::with_backend(|be| layout::ensure_system(be, now()));
            db
        }
        None => bootstrap(),
    };
    if let Some(u) = lone_passwordless(&db) {
        let s = snapshot(&db, u);
        crate::serial_println!("accounts: signed in {} (no password set)", s.name);
        set_session(Some(s));
    }
    if let Ok(mut g) = DB.lock() {
        *g = Some(db);
    }
    if let Ok(mut g) = GUARD.lock() {
        *g = Some(LoginGuard::new());
    }
}

fn bootstrap() -> AccountDb {
    let mut db = AccountDb::system();
    let _ = db.add_user(DEFAULT_USER, "Kitsune", "", true);
    let user = db.user(DEFAULT_USER).cloned();
    if let Some(user) = user {
        let t = now();
        let r = vfs::root::with_backend(|be| {
            layout::ensure_system(be, t)?;
            layout::adopt_legacy(be, &user, &[], t)
        });
        match r {
            Ok(Ok(a)) => crate::serial_println!(
                "accounts: first boot: created {} and moved {} + {} legacy entries into {}",
                DEFAULT_USER,
                a.moved,
                a.from_home,
                user.home
            ),
            _ => crate::serial_println!("accounts: first boot: could not prepare the folders"),
        }
    }
    if !save(&db) {
        crate::serial_println!("accounts: could not write {}", "/etc/accounts");
    }
    db
}

/// The one user that signs in without asking: the only account besides `root`, with no password.
fn lone_passwordless(db: &AccountDb) -> Option<&User> {
    let mut it = db.users().iter().filter(|u| u.uid != 0);
    let first = it.next()?;
    (it.next().is_none() && first.has_no_password()).then_some(first)
}

// ---- the session ------------------------------------------------------------------------

/// The signed-in user, if any.
pub fn current() -> Option<Session> {
    SESSION.lock().ok().and_then(|g| g.clone())
}

/// The credentials `vfs` enforces; `None` means "no session": full rights (early boot).
pub fn cred() -> Option<Cred> {
    SESSION
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|s| s.cred.clone()))
}

/// Does the signed-in account have a password (so locking the screen means something)?
pub fn current_has_password() -> bool {
    current().is_some_and(|s| {
        with_db(|db| db.user(&s.name).is_some_and(|u| !u.has_no_password())).unwrap_or(false)
    })
}

/// Is somebody signed in?
pub fn signed_in() -> bool {
    current().is_some()
}

/// The signed-in user's home folder (`/home` before anyone signs in).
pub fn home() -> Vec<u8> {
    current().map_or_else(|| b"/home".to_vec(), |s| s.home.into_bytes())
}

/// Make `s` the session (or end it) and point the file manager's sidebar at its home.
fn set_session(s: Option<Session>) {
    kitsune_core::homes::set_current_home(s.as_ref().map_or(&[][..], |s| s.home.as_bytes()));
    if let Ok(mut g) = SESSION.lock() {
        *g = s;
    }
}

/// Sign out. The desktop then shows the login screen.
pub fn logout() {
    if let Some(s) = current() {
        crate::klog!(Info, "auth: {}: signed out", s.name);
    }
    set_session(None);
}

/// Check a name and password and, when they are right, make it the session.
pub fn login(name: &str, password: &str) -> Result<(), LoginFail> {
    let t = now_ms();
    {
        let mut g = GUARD.lock().map_err(|_| LoginFail::Unknown)?;
        if let Some(guard) = g.as_mut()
            && let Err(th) = guard.check(name, t)
        {
            crate::klog!(
                Warn,
                "auth: {name}: sign-in held back for {} ms",
                th.retry_in_ms
            );
            return Err(LoginFail::Throttled(th.retry_in_ms));
        }
    }
    let attempt = with_db(|db| {
        db.authenticate(name, password)
            .map(|u| (snapshot(db, u), u.password.clone()))
    })
    .ok_or(LoginFail::Unknown)?;
    match attempt {
        Ok((session, stored)) => {
            if let Ok(mut g) = GUARD.lock()
                && let Some(guard) = g.as_mut()
            {
                guard.succeeded(name);
            }
            // A hash made with an older, cheaper setting is renewed now that the password is known.
            if password::needs_rehash(&stored) {
                let _ = rehash(name, password);
            }
            set_session(Some(session));
            crate::klog!(
                Info,
                "auth: {name}: signed in ({} ms)",
                now_ms().saturating_sub(t)
            );
            Ok(())
        }
        Err(e) => {
            crate::klog!(
                Warn,
                "auth: {name}: sign-in refused ({})",
                match e {
                    AuthError::UnknownUser => "unknown account",
                    AuthError::BadPassword => "wrong password",
                    AuthError::Locked => "account disabled",
                }
            );
            if let Ok(mut g) = GUARD.lock()
                && let Some(guard) = g.as_mut()
            {
                guard.failed(name, now_ms());
            }
            Err(match e {
                AuthError::UnknownUser => LoginFail::Unknown,
                AuthError::BadPassword => LoginFail::BadPassword,
                AuthError::Locked => LoginFail::Locked,
            })
        }
    }
}

fn hash_for(password: &str) -> Result<String, password::PasswordError> {
    let mut salt = [0u8; SALT_LEN];
    crate::rng::fill(&mut salt);
    password::hash(password, &salt)
}

fn rehash(name: &str, pw: &str) -> bool {
    let Ok(h) = hash_for(pw) else { return false };
    with_db(|db| {
        if db.set_password(name, &h).is_ok() {
            save(db)
        } else {
            false
        }
    })
    .unwrap_or(false)
}

// ---- the account list and its changes ------------------------------------------------------

/// Every account except `root`, for the Users page and the login screen.
pub fn users() -> Vec<UserRow> {
    with_db(|db| {
        db.users()
            .iter()
            .filter(|u| u.uid != 0)
            .map(|u| UserRow {
                name: u.name.clone(),
                full_name: u.full_name.clone(),
                uid: u.uid,
                admin: db.is_admin(u),
                locked: u.is_locked(),
                has_password: !u.has_no_password() && !u.is_locked(),
            })
            .collect()
    })
    .unwrap_or_default()
}

/// Does the desktop have to ask who is signing in? (False while a lone account has no password.)
pub fn login_required() -> bool {
    with_db(|db| lone_passwordless(db).is_none()).unwrap_or(false)
}

/// Write the database and, when that worked, record `what` in the log (names only, never a
/// password) as `auth: <who> <what>`.
fn persist(what: core::fmt::Arguments<'_>) -> Result<(), ChangeFail> {
    if with_db(|db| save(db)).unwrap_or(false) {
        let who = current().map_or_else(|| String::from("?"), |s| s.name);
        crate::klog!(Info, "auth: {who}: {what}");
        Ok(())
    } else {
        Err(ChangeFail::Storage)
    }
}

fn require_admin() -> Result<Session, ChangeFail> {
    match current() {
        Some(s) if s.admin => Ok(s),
        _ => Err(ChangeFail::Denied),
    }
}

/// Add an account (administrators only). `password` may be empty for none.
pub fn add_user(
    name: &str,
    full_name: &str,
    password: &str,
    admin: bool,
) -> Result<(), ChangeFail> {
    require_admin()?;
    let hash = if password.is_empty() {
        String::new()
    } else {
        hash_for(password).map_err(ChangeFail::Password)?
    };
    let user = with_db(|db| {
        db.add_user(name, full_name, &hash, admin)
            .map(|_| db.user(name).cloned())
    })
    .ok_or(ChangeFail::Storage)?
    .map_err(ChangeFail::Account)?
    .ok_or(ChangeFail::Storage)?;
    let t = now();
    let _ = vfs::root::with_backend(|be| layout::create_home(be, &user, t));
    let r = persist(format_args!(
        "created the account {name}{}",
        if admin { " (administrator)" } else { "" }
    ));
    vfs::root::touch();
    r
}

/// Change a password. Administrators may set anyone's; anybody may change their own by giving
/// the current one (when it has one). `new` empty removes the password.
pub fn set_password(name: &str, current_pw: &str, new: &str) -> Result<(), ChangeFail> {
    let me = current().ok_or(ChangeFail::Denied)?;
    if !me.admin && me.name != name {
        return Err(ChangeFail::Denied);
    }
    if me.name == name {
        // Even an administrator proves who they are before changing their own password.
        let known = with_db(|db| {
            db.user(name)
                .map(|u| (u.has_no_password(), u.password.clone()))
        })
        .flatten()
        .ok_or(ChangeFail::Account(AccountError::NoSuchUser))?;
        if !known.0 && !password::verify(current_pw, &known.1) {
            return Err(ChangeFail::BadPassword);
        }
    }
    let hash = if new.is_empty() {
        String::new()
    } else {
        hash_for(new).map_err(ChangeFail::Password)?
    };
    with_db(|db| db.set_password(name, &hash))
        .ok_or(ChangeFail::Storage)?
        .map_err(ChangeFail::Account)?;
    persist(format_args!("changed the password of {name}"))
}

/// Make `name` an administrator or not (administrators only).
pub fn set_admin(name: &str, admin: bool) -> Result<(), ChangeFail> {
    require_admin()?;
    let r = with_db(|db| {
        if admin {
            db.add_to_group(name, kitsune_core::account::GID_ADMIN)
        } else {
            db.remove_from_group(name, kitsune_core::account::GID_ADMIN)
        }
    })
    .ok_or(ChangeFail::Storage)?;
    r.map_err(ChangeFail::Account)?;
    persist(format_args!(
        "{} {name} as administrator",
        if admin { "made" } else { "unmade" }
    ))
}

/// Delete an account (administrators only; not oneself). Its home folder goes too when
/// `delete_files`, otherwise it stays on the disk for an administrator to deal with.
pub fn remove_user(name: &str, delete_files: bool) -> Result<(), ChangeFail> {
    let me = require_admin()?;
    if me.name == name {
        return Err(ChangeFail::Account(AccountError::Protected));
    }
    let home = with_db(|db| db.user(name).map(|u| u.home.clone()))
        .flatten()
        .ok_or(ChangeFail::Account(AccountError::NoSuchUser))?;
    with_db(|db| db.remove_user(name))
        .ok_or(ChangeFail::Storage)?
        .map_err(ChangeFail::Account)?;
    if delete_files {
        let _ = vfs::root::purge(home.as_bytes());
    }
    persist(format_args!(
        "removed the account {name}{}",
        if delete_files { " and its files" } else { "" }
    ))
}

/// `name`'s display text: the full name when there is one.
pub fn display_name(row: &UserRow) -> &str {
    if row.full_name.is_empty() {
        &row.name
    } else {
        &row.full_name
    }
}

/// uid -> name for listings (`"?"` when unknown).
pub fn user_name(uid: u32) -> String {
    if uid == 0 {
        return "root".to_string();
    }
    with_db(|db| db.user_name(uid).to_string()).unwrap_or_else(|| "?".to_string())
}

/// gid -> name for listings.
pub fn group_name(gid: u32) -> String {
    with_db(|db| db.group_name(gid).to_string()).unwrap_or_else(|| "?".to_string())
}

// ---- lookups for the shell and the file manager ----------------------------------------------

/// uid of user `name`.
pub fn lookup_user(name: &str) -> Option<u32> {
    with_db(|db| db.user(name).map(|u| u.uid)).flatten()
}

/// gid of group `name`.
pub fn lookup_group(name: &str) -> Option<u32> {
    with_db(|db| db.group(name).map(|g| g.gid)).flatten()
}

/// The signed-in user as the shell reports it (`whoami`, `id`).
pub fn identity() -> Option<kitsune_core::shell::sys::Identity> {
    let s = current()?;
    with_db(|db| {
        let gname = db.group_name(s.cred.gid).to_string();
        let mut groups = alloc::vec![(s.cred.gid, gname.clone())];
        for g in &s.cred.groups {
            if *g != s.cred.gid {
                groups.push((*g, db.group_name(*g).to_string()));
            }
        }
        kitsune_core::shell::sys::Identity {
            name: s.name.clone(),
            uid: s.uid,
            gid: s.cred.gid,
            gname,
            groups,
            admin: s.admin,
        }
    })
}

/// Accounts exist but nobody is signed in: the screen is on the sign-in layer. The user-facing
/// file access is refused in this state (see `vfs`).
pub fn locked_out() -> bool {
    let has_db = DB.lock().ok().is_some_and(|g| g.is_some());
    has_db && !signed_in()
}
