//! Where secrets come from, without the engine learning what they are.
//!
//! A [`CredentialStore`] is a host capability, injected the way an
//! `HttpClient` is: Keychain on Apple platforms, Keystore on Android,
//! Secret Service or an environment variable on a desktop, nothing at all
//! in a browser. This module holds the types a shell must name; the
//! implementations live with the host.
//!
//! # Three decisions that are hard to change later
//!
//! The store is small on purpose. Almost everything about authentication is
//! deliberately *not* here, because every piece that crosses into the engine
//! is a piece a future C ABI has to keep holding still.
//!
//! 1. **The value is an opaque `Authorization` header.** Not a username and
//!    a password. `opds-client` has always stored a precomputed header
//!    string internally, and naming the scheme in this type would be the
//!    one mistake that costs an ABI break: HTTP Basic today, OAuth bearer
//!    tokens and per-user API keys tomorrow, all of which are a header
//!    value and differ in nothing the engine can act on. A host that
//!    genuinely cannot express its scheme as one header — per-request
//!    signing, cookie sessions — overrides the *transport* instead, which
//!    is what `HttpClient` is for.
//!
//! 2. **The key is stable and not a secret.** Catalog URLs may carry
//!    per-user API keys in the path (opds-client's `INTEROP.md` §4), so keying
//!    on the full URL would make the key unloggable *and* break every
//!    stored credential when a catalog relocates. [`CredentialKey`] is an
//!    origin or a library row id. That matters more than it looks: a key
//!    change orphans entries already written to a device's Keychain, and
//!    no compiler catches it.
//!
//! 3. **Lookups can be repeated, and can say why they failed.**
//!    [`Freshness`] lets a caller say "the value you gave me was rejected",
//!    which is the difference between a scheme whose secret is constant and
//!    one whose token expires. [`CredentialLookup`] distinguishes *nothing
//!    stored* from *locked right now* — a Keychain item before first
//!    unlock, a key bound to biometrics that have not been satisfied —
//!    because collapsing those into `None` turns a temporary condition into
//!    a permanent-looking one and invites the shell to re-prompt for
//!    something the user already has.
//!
//! # What is not here, on purpose
//!
//! **Prompting.** No method on this trait may put up a dialog. Android's
//! user-authentication-bound keys prompt on the UI thread, and chapbook
//! reaches credentials from the loader thread (`chapbook-reader`'s
//! "viewers never call `unit_bytes`/`resource` on the UI thread"), so a
//! store that can block on a person is a deadlock waiting for a slow user.
//! `opds-client` already declines to retry on its own and hands back
//! `AuthRequired` with the server's Authentication Document; the shell
//! prompts, then calls [`CredentialStore::store`]. That keeps OAuth's
//! browser round-trip — `ASWebAuthenticationSession`, a Custom Tab —
//! entirely outside the engine, where it costs the boundary nothing.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::ChapbookError;

/// A stable, non-secret name for one set of credentials.
///
/// Two constructors, because there are two things worth keying on: a
/// catalog the library knows about (use the row id — it survives the
/// catalog moving) and a bare URL a session was handed (use the origin —
/// the path may be secret, the origin is not).
///
/// The string form is what a host passes to its own store, so it is part of
/// the contract: `kSecAttrService` on Apple platforms, the alias of a
/// Keystore entry on Android. Changing how it is derived strands
/// credentials that are already on a device.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CredentialKey(String);

impl CredentialKey {
    /// The credentials for a catalog stored in the library, keyed by row id.
    ///
    /// Preferred over [`Self::http_origin`] when the row exists: it stays
    /// put when the catalog's URL changes, and it distinguishes two
    /// accounts on the same server.
    pub fn opds_source(id: i64) -> CredentialKey {
        CredentialKey(format!("opds/source/{id}"))
    }

    /// The credentials for a URL's origin — scheme, host and any explicit
    /// port, with the path deliberately discarded.
    ///
    /// Returns `None` for anything that does not parse as `scheme://host`.
    /// The discarded path is the point: a per-user API key living there
    /// must not end up in a key that gets logged, and a catalog that moves
    /// `/opds/` to `/opds/v2/` must not lose its stored login.
    pub fn http_origin(url: &str) -> Option<CredentialKey> {
        let (scheme, rest) = url.split_once("://")?;
        if scheme.is_empty() {
            return None;
        }
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        // Strip `user:pass@`, which is credentials rather than identity.
        let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        if host.is_empty() {
            return None;
        }
        Some(CredentialKey(format!(
            "opds/origin/{}://{}",
            scheme.to_ascii_lowercase(),
            host.to_ascii_lowercase()
        )))
    }

    /// A key the host derives itself, for a shell whose accounts do not
    /// correspond to either of the above.
    pub fn from_raw(key: impl Into<String>) -> CredentialKey {
        CredentialKey(key.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for CredentialKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One credential: a complete `Authorization` header value, plus a
/// non-secret label for the account it belongs to.
///
/// The header value is opaque here by design — see this module's header.
/// Build the HTTP Basic form with [`basic_authorization`].
#[derive(Clone, PartialEq, Eq)]
pub struct Credential {
    /// The complete header value, scheme word included: `Basic dXNlcjpwdw==`,
    /// `Bearer eyJ...`. Sent verbatim as `Authorization`.
    pub authorization: String,
    /// Who this is, for display in a settings screen. Never a secret, and
    /// never sent anywhere.
    pub account: Option<String>,
}

impl Credential {
    pub fn new(authorization: impl Into<String>) -> Credential {
        Credential {
            authorization: authorization.into(),
            account: None,
        }
    }

    /// HTTP Basic from a username and password.
    pub fn basic(username: &str, password: &str) -> Credential {
        Credential {
            authorization: basic_authorization(username, password),
            account: Some(username.to_string()),
        }
    }

    pub fn with_account(mut self, account: impl Into<String>) -> Credential {
        self.account = Some(account.into());
        self
    }
}

/// Redacted: the whole value is a secret, and an error log that prints a
/// session's config must not become a credential dump.
impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let scheme = self
            .authorization
            .split_once(' ')
            .map_or("?", |(scheme, _)| scheme);
        f.debug_struct("Credential")
            .field("authorization", &format_args!("{scheme} <redacted>"))
            .field("account", &self.account)
            .finish()
    }
}

/// The `Authorization` header value for HTTP Basic.
///
/// Public because a shell that prompts for a username and password needs to
/// produce one before calling [`CredentialStore::store`], and because the
/// alternative — a `set_basic_auth` that hides it — is what makes a
/// credential type scheme-specific.
pub fn basic_authorization(username: &str, password: &str) -> String {
    format!(
        "Basic {}",
        base64(format!("{username}:{password}").as_bytes())
    )
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = u32::from_be_bytes([0, b[0], b[1], b[2]]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i)) as usize & 0x3f] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// How current the caller needs the answer to be.
///
/// The whole reason this parameter exists is that a store's cheapest answer
/// and its correct answer differ once a scheme has expiring secrets. HTTP
/// Basic ignores it; a bearer token does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// Whatever is stored. The ordinary case.
    Cached,
    /// The last value handed out was rejected by the server. Do not return
    /// the same bytes from a cache; refresh if the scheme can, and report
    /// [`CredentialLookup::Missing`] if it cannot.
    Renewed,
}

/// What a lookup found.
///
/// [`Missing`](Self::Missing) and [`Locked`](Self::Locked) are separate
/// because a shell should re-prompt for the first and wait for the second —
/// a Keychain item is unreadable before first unlock, and treating that as
/// "no credentials" asks the user to type a password they already gave.
#[derive(Debug, Clone)]
pub enum CredentialLookup {
    Found(Credential),
    /// Nothing is stored for this key. Prompt.
    Missing,
    /// Something is stored but cannot be read right now — device locked,
    /// user authentication not satisfied. Do not prompt for a new one.
    Locked,
    /// The store itself failed. The string is for logs, not for users.
    Failed(String),
}

/// A host's secret storage, injected into a session.
///
/// `Send + Sync` because lookups happen on the loader thread. No method may
/// prompt — see this module's header for why that is a hard rule rather
/// than a preference.
pub trait CredentialStore: Send + Sync {
    /// Look up the credential for `key`.
    ///
    /// Called more than once per session, and called again with
    /// [`Freshness::Renewed`] after a 401, so an implementation must not
    /// treat the first call as the only one.
    fn get(&self, key: &CredentialKey, freshness: Freshness) -> CredentialLookup;

    /// Persist a credential the shell obtained by prompting.
    ///
    /// The default refuses, which is right for a read-only store like
    /// [`EnvCredentials`] and wrong for anything a shell prompts into.
    fn store(&self, key: &CredentialKey, credential: &Credential) -> Result<(), ChapbookError> {
        let _ = (key, credential);
        Err(ChapbookError::Credential(
            "this credential store is read-only".into(),
        ))
    }

    /// Forget the credential for `key`, if any. Succeeds when there was
    /// nothing to forget.
    fn forget(&self, key: &CredentialKey) -> Result<(), ChapbookError> {
        let _ = key;
        Err(ChapbookError::Credential(
            "this credential store is read-only".into(),
        ))
    }
}

/// A store that holds nothing.
///
/// The right default for a session whose caller has not said otherwise, and
/// the only honest answer on wasm, where the platform has no secret storage
/// to offer. Authenticated catalogs then fail with the server's
/// Authentication Document, which is what a shell needs to prompt.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoCredentials;

impl CredentialStore for NoCredentials {
    fn get(&self, _key: &CredentialKey, _freshness: Freshness) -> CredentialLookup {
        CredentialLookup::Missing
    }
}

/// Credentials from `CHAPBOOK_OPDS_USER` and `CHAPBOOK_OPDS_PASSWORD`.
///
/// One pair for every key, because environment variables have no room for a
/// second catalog. This is what the CLI and the reference viewers use, and
/// what a headless box — a server, a Raspberry Pi with no Secret Service —
/// can rely on. It is deliberately the *only* persistent store shipped
/// here: a plaintext-on-disk store would be the convenient thing to reach
/// for and would put the secrets back where this extraction took them from.
#[derive(Debug, Default, Clone, Copy)]
pub struct EnvCredentials;

impl EnvCredentials {
    pub const USER_VAR: &'static str = "CHAPBOOK_OPDS_USER";
    pub const PASSWORD_VAR: &'static str = "CHAPBOOK_OPDS_PASSWORD";

    pub fn new() -> EnvCredentials {
        EnvCredentials
    }
}

impl CredentialStore for EnvCredentials {
    fn get(&self, _key: &CredentialKey, freshness: Freshness) -> CredentialLookup {
        // A rejected value cannot be refreshed: the environment will say
        // the same thing next time, and returning it again would spend a
        // retry to earn the same 401.
        if freshness == Freshness::Renewed {
            return CredentialLookup::Missing;
        }
        match (
            std::env::var(Self::USER_VAR),
            std::env::var(Self::PASSWORD_VAR),
        ) {
            (Ok(user), Ok(password)) => {
                CredentialLookup::Found(Credential::basic(&user, &password))
            }
            _ => CredentialLookup::Missing,
        }
    }
}

/// An in-process store, for tests and for shells that prompt once per
/// launch and deliberately keep nothing on disk.
#[derive(Debug, Default)]
pub struct MemoryCredentials {
    entries: Mutex<HashMap<CredentialKey, Credential>>,
}

impl MemoryCredentials {
    pub fn new() -> MemoryCredentials {
        MemoryCredentials::default()
    }
}

impl CredentialStore for MemoryCredentials {
    fn get(&self, key: &CredentialKey, _freshness: Freshness) -> CredentialLookup {
        // `Renewed` is honored by *not* having anything newer: whatever the
        // shell last stored is the freshest this store can be, so a
        // rejected value stays rejected until the shell prompts again.
        match self.entries.lock() {
            Ok(entries) => match entries.get(key) {
                Some(credential) => CredentialLookup::Found(credential.clone()),
                None => CredentialLookup::Missing,
            },
            Err(_) => CredentialLookup::Failed("credential store poisoned".into()),
        }
    }

    fn store(&self, key: &CredentialKey, credential: &Credential) -> Result<(), ChapbookError> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| ChapbookError::Credential("credential store poisoned".into()))?;
        entries.insert(key.clone(), credential.clone());
        Ok(())
    }

    fn forget(&self, key: &CredentialKey) -> Result<(), ChapbookError> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| ChapbookError::Credential("credential store poisoned".into()))?;
        entries.remove(key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_matches_rfc_7617_example() {
        // RFC 7617 §2: "Aladdin:open sesame".
        assert_eq!(
            basic_authorization("Aladdin", "open sesame"),
            "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ=="
        );
        // Padding for both remainders.
        assert_eq!(basic_authorization("a", "b"), "Basic YTpi");
        assert_eq!(basic_authorization("ab", "cd"), "Basic YWI6Y2Q=");
    }

    #[test]
    fn origin_key_drops_the_secret_bearing_path() {
        let key = CredentialKey::http_origin("https://cat.example.com/opds/abc123secret/").unwrap();
        assert_eq!(key.as_str(), "opds/origin/https://cat.example.com");
        assert!(!key.as_str().contains("abc123secret"));
    }

    #[test]
    fn origin_key_is_stable_across_paths_and_case() {
        let a = CredentialKey::http_origin("https://Cat.Example.com/opds/").unwrap();
        let b = CredentialKey::http_origin("https://cat.example.com/opds/v2/?q=x").unwrap();
        assert_eq!(a, b, "a catalog moving its path keeps its login");
    }

    #[test]
    fn origin_key_keeps_port_and_drops_userinfo() {
        let key = CredentialKey::http_origin("http://user:pw@host:8080/feed").unwrap();
        assert_eq!(key.as_str(), "opds/origin/http://host:8080");
    }

    #[test]
    fn origin_key_rejects_non_urls() {
        assert!(CredentialKey::http_origin("/books/a.epub").is_none());
        assert!(CredentialKey::http_origin("https://").is_none());
    }

    #[test]
    fn credential_debug_redacts_the_secret() {
        let credential = Credential::basic("Aladdin", "open sesame");
        let shown = format!("{credential:?}");
        assert!(shown.contains("Basic <redacted>"), "{shown}");
        assert!(!shown.contains("QWxhZGRpbjpvcGVuIHNlc2FtZQ=="), "{shown}");
        assert!(
            shown.contains("Aladdin"),
            "the account label is not a secret"
        );
    }

    #[test]
    fn memory_store_roundtrips_and_forgets() {
        let store = MemoryCredentials::new();
        let key = CredentialKey::opds_source(7);
        assert!(matches!(
            store.get(&key, Freshness::Cached),
            CredentialLookup::Missing
        ));
        store.store(&key, &Credential::basic("u", "p")).unwrap();
        let CredentialLookup::Found(found) = store.get(&key, Freshness::Cached) else {
            panic!("stored credential should come back");
        };
        assert_eq!(found.authorization, basic_authorization("u", "p"));
        store.forget(&key).unwrap();
        assert!(matches!(
            store.get(&key, Freshness::Cached),
            CredentialLookup::Missing
        ));
    }

    #[test]
    fn no_credentials_is_read_only() {
        let store = NoCredentials;
        let key = CredentialKey::opds_source(1);
        assert!(matches!(
            store.get(&key, Freshness::Cached),
            CredentialLookup::Missing
        ));
        assert!(store.store(&key, &Credential::new("Bearer x")).is_err());
    }

    #[test]
    fn env_store_declines_to_repeat_a_rejected_value() {
        // Independent of whether the variables are set: the environment
        // cannot produce anything newer than what it already said, so a
        // `Renewed` lookup must not hand back the value the server just
        // refused. Otherwise the retry spends a request earning the same
        // 401.
        let store = EnvCredentials::new();
        assert!(matches!(
            store.get(&CredentialKey::opds_source(1), Freshness::Renewed),
            CredentialLookup::Missing
        ));
    }

    #[test]
    fn store_is_object_safe() {
        // The C ABI hands this across as a vtable behind a pointer; if the
        // trait stops being object-safe that boundary stops existing.
        let store: std::sync::Arc<dyn CredentialStore> = std::sync::Arc::new(NoCredentials);
        assert!(matches!(
            store.get(&CredentialKey::from_raw("k"), Freshness::Cached),
            CredentialLookup::Missing
        ));
    }
}
