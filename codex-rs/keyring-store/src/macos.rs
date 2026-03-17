use security_framework::base::Error as SecurityError;
use security_framework::passwords::PasswordOptions;
use security_framework::passwords::delete_generic_password_options;
use security_framework::passwords::generic_password;
use security_framework::passwords::set_generic_password_options;
use security_framework_sys::base::errSecItemNotFound;
use std::fmt;
use tracing::warn;

#[derive(Debug, Clone)]
pub(crate) struct MacOsAccessGroupError {
    details: String,
}

impl MacOsAccessGroupError {
    fn new(details: impl Into<String>) -> Self {
        Self {
            details: details.into(),
        }
    }
}

impl fmt::Display for MacOsAccessGroupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "failed to access macOS keychain item in shared access group: {}",
            self.details
        )
    }
}

impl std::error::Error for MacOsAccessGroupError {}

const SHARED_ACCESS_GROUP: &str = "2DC432GLL2.com.openai.codex.shared";

pub(crate) fn load_password(
    service: &str,
    account: &str,
) -> Result<Option<String>, MacOsAccessGroupError> {
    load_password_with_store(&NativePasswordStore, service, account)
}

pub(crate) fn save_password(
    service: &str,
    account: &str,
    value: &str,
) -> Result<(), MacOsAccessGroupError> {
    save_password_with_store(&NativePasswordStore, service, account, value)
}

pub(crate) fn delete_password(service: &str, account: &str) -> Result<bool, MacOsAccessGroupError> {
    delete_password_with_store(&NativePasswordStore, service, account)
}

trait PasswordStore {
    fn load_shared_password_bytes(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Option<Vec<u8>>, MacOsAccessGroupError>;

    fn load_legacy_password_bytes(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Option<Vec<u8>>, MacOsAccessGroupError>;

    fn save_shared_password_bytes(
        &self,
        service: &str,
        account: &str,
        value: &[u8],
    ) -> Result<(), MacOsAccessGroupError>;

    fn delete_shared_password(
        &self,
        service: &str,
        account: &str,
    ) -> Result<bool, MacOsAccessGroupError>;

    fn delete_legacy_password(
        &self,
        service: &str,
        account: &str,
    ) -> Result<bool, MacOsAccessGroupError>;
}

#[derive(Debug)]
struct NativePasswordStore;

impl PasswordStore for NativePasswordStore {
    fn load_shared_password_bytes(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Option<Vec<u8>>, MacOsAccessGroupError> {
        load_password_bytes(
            shared_password_options(service, account),
            "read shared generic password",
        )
    }

    fn load_legacy_password_bytes(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Option<Vec<u8>>, MacOsAccessGroupError> {
        load_password_bytes(
            legacy_password_options(service, account),
            "read legacy generic password",
        )
    }

    fn save_shared_password_bytes(
        &self,
        service: &str,
        account: &str,
        value: &[u8],
    ) -> Result<(), MacOsAccessGroupError> {
        set_generic_password_options(value, shared_password_options(service, account))
            .map_err(|error| wrap_security_error("write shared generic password", error))
    }

    fn delete_shared_password(
        &self,
        service: &str,
        account: &str,
    ) -> Result<bool, MacOsAccessGroupError> {
        delete_password_options(
            shared_password_options(service, account),
            "delete shared generic password",
        )
    }

    fn delete_legacy_password(
        &self,
        service: &str,
        account: &str,
    ) -> Result<bool, MacOsAccessGroupError> {
        delete_password_options(
            legacy_password_options(service, account),
            "delete legacy generic password",
        )
    }
}

fn load_password_with_store(
    store: &impl PasswordStore,
    service: &str,
    account: &str,
) -> Result<Option<String>, MacOsAccessGroupError> {
    if let Some(password) = store.load_shared_password_bytes(service, account)? {
        return decode_password(password, service, account).map(Some);
    }

    let Some(password) = store.load_legacy_password_bytes(service, account)? else {
        return Ok(None);
    };

    if let Err(error) = store.save_shared_password_bytes(service, account, &password) {
        warn!(
            error = %error,
            service,
            account,
            "failed to backfill legacy macOS keychain item into the shared access group"
        );
    }

    decode_password(password, service, account).map(Some)
}

fn save_password_with_store(
    store: &impl PasswordStore,
    service: &str,
    account: &str,
    value: &str,
) -> Result<(), MacOsAccessGroupError> {
    store.save_shared_password_bytes(service, account, value.as_bytes())
}

fn delete_password_with_store(
    store: &impl PasswordStore,
    service: &str,
    account: &str,
) -> Result<bool, MacOsAccessGroupError> {
    let shared_removed = store.delete_shared_password(service, account)?;
    let legacy_removed = store.delete_legacy_password(service, account)?;
    Ok(shared_removed || legacy_removed)
}

fn decode_password(
    password: Vec<u8>,
    service: &str,
    account: &str,
) -> Result<String, MacOsAccessGroupError> {
    String::from_utf8(password).map_err(|error| {
        MacOsAccessGroupError::new(format!(
            "decode password for service={service}, account={account}: {error}"
        ))
    })
}

fn load_password_bytes(
    options: PasswordOptions,
    action: &str,
) -> Result<Option<Vec<u8>>, MacOsAccessGroupError> {
    match generic_password(options) {
        Ok(password) => Ok(Some(password)),
        Err(error) if error.code() == errSecItemNotFound => Ok(None),
        Err(error) => Err(wrap_security_error(action, error)),
    }
}

fn delete_password_options(
    options: PasswordOptions,
    action: &str,
) -> Result<bool, MacOsAccessGroupError> {
    match delete_generic_password_options(options) {
        Ok(()) => Ok(true),
        Err(error) if error.code() == errSecItemNotFound => Ok(false),
        Err(error) => Err(wrap_security_error(action, error)),
    }
}

fn shared_password_options(service: &str, account: &str) -> PasswordOptions {
    let mut options = PasswordOptions::new_generic_password(service, account);
    options.use_protected_keychain();
    options.set_access_group(SHARED_ACCESS_GROUP);
    options
}

fn legacy_password_options(service: &str, account: &str) -> PasswordOptions {
    PasswordOptions::new_generic_password(service, account)
}

fn wrap_security_error(action: &str, error: SecurityError) -> MacOsAccessGroupError {
    MacOsAccessGroupError::new(format!("{action}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::cell::RefCell;

    #[derive(Default)]
    struct MockPasswordStore {
        shared: RefCell<Option<Vec<u8>>>,
        legacy: RefCell<Option<Vec<u8>>>,
        save_shared_error: RefCell<Option<MacOsAccessGroupError>>,
        shared_writes: Cell<usize>,
    }

    impl MockPasswordStore {
        fn with_shared_password(self, value: &str) -> Self {
            self.shared.replace(Some(value.as_bytes().to_vec()));
            self
        }

        fn with_legacy_password(self, value: &str) -> Self {
            self.legacy.replace(Some(value.as_bytes().to_vec()));
            self
        }

        fn with_save_shared_error(self, details: &str) -> Self {
            self.save_shared_error
                .replace(Some(MacOsAccessGroupError::new(details)));
            self
        }
    }

    impl PasswordStore for MockPasswordStore {
        fn load_shared_password_bytes(
            &self,
            _service: &str,
            _account: &str,
        ) -> Result<Option<Vec<u8>>, MacOsAccessGroupError> {
            Ok(self.shared.borrow().clone())
        }

        fn load_legacy_password_bytes(
            &self,
            _service: &str,
            _account: &str,
        ) -> Result<Option<Vec<u8>>, MacOsAccessGroupError> {
            Ok(self.legacy.borrow().clone())
        }

        fn save_shared_password_bytes(
            &self,
            _service: &str,
            _account: &str,
            value: &[u8],
        ) -> Result<(), MacOsAccessGroupError> {
            if let Some(error) = self.save_shared_error.borrow().clone() {
                return Err(error);
            }

            self.shared.replace(Some(value.to_vec()));
            self.shared_writes.set(self.shared_writes.get() + 1);
            Ok(())
        }

        fn delete_shared_password(
            &self,
            _service: &str,
            _account: &str,
        ) -> Result<bool, MacOsAccessGroupError> {
            Ok(self.shared.borrow_mut().take().is_some())
        }

        fn delete_legacy_password(
            &self,
            _service: &str,
            _account: &str,
        ) -> Result<bool, MacOsAccessGroupError> {
            Ok(self.legacy.borrow_mut().take().is_some())
        }
    }

    #[test]
    fn load_prefers_shared_password_when_present() {
        let store = MockPasswordStore::default()
            .with_shared_password("shared-value")
            .with_legacy_password("legacy-value");

        let password = load_password_with_store(&store, "Codex Auth", "greg")
            .unwrap()
            .expect("expected password");

        assert_eq!(password, "shared-value");
        assert_eq!(
            store.shared.borrow().clone(),
            Some(b"shared-value".to_vec())
        );
        assert_eq!(
            store.legacy.borrow().clone(),
            Some(b"legacy-value".to_vec())
        );
        assert_eq!(store.shared_writes.get(), 0);
    }

    #[test]
    fn load_backfills_shared_password_from_legacy_without_deleting_legacy() {
        let store = MockPasswordStore::default().with_legacy_password("legacy-value");

        let password = load_password_with_store(&store, "Codex Auth", "greg")
            .unwrap()
            .expect("expected password");

        assert_eq!(password, "legacy-value");
        assert_eq!(
            store.shared.borrow().clone(),
            Some(b"legacy-value".to_vec())
        );
        assert_eq!(
            store.legacy.borrow().clone(),
            Some(b"legacy-value".to_vec())
        );
        assert_eq!(store.shared_writes.get(), 1);
    }

    #[test]
    fn load_returns_legacy_password_when_shared_backfill_fails() {
        let store = MockPasswordStore::default()
            .with_legacy_password("legacy-value")
            .with_save_shared_error("shared write failed");

        let password = load_password_with_store(&store, "Codex Auth", "greg")
            .unwrap()
            .expect("expected password");

        assert_eq!(password, "legacy-value");
        assert_eq!(store.shared.borrow().clone(), None);
        assert_eq!(
            store.legacy.borrow().clone(),
            Some(b"legacy-value".to_vec())
        );
        assert_eq!(store.shared_writes.get(), 0);
    }

    #[test]
    fn save_writes_only_to_shared_password() {
        let store = MockPasswordStore::default().with_legacy_password("legacy-value");

        save_password_with_store(&store, "Codex Auth", "greg", "shared-value").unwrap();

        assert_eq!(
            store.shared.borrow().clone(),
            Some(b"shared-value".to_vec())
        );
        assert_eq!(
            store.legacy.borrow().clone(),
            Some(b"legacy-value".to_vec())
        );
        assert_eq!(store.shared_writes.get(), 1);
    }

    #[test]
    fn delete_removes_both_shared_and_legacy_passwords() {
        let store = MockPasswordStore::default()
            .with_shared_password("shared-value")
            .with_legacy_password("legacy-value");

        let deleted = delete_password_with_store(&store, "Codex Auth", "greg").unwrap();

        assert!(deleted);
        assert_eq!(store.shared.borrow().clone(), None);
        assert_eq!(store.legacy.borrow().clone(), None);
    }

    #[test]
    fn shared_access_group_matches_configured_team_prefix() {
        assert_eq!(SHARED_ACCESS_GROUP, "2DC432GLL2.com.openai.codex.shared");
    }
}
