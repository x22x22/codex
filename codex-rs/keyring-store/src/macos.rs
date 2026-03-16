use core_foundation::base::TCFType;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;
use security_framework::base::Error as SecurityError;
use security_framework::item::ItemClass;
use security_framework::item::ItemSearchOptions;
use security_framework::item::ItemUpdateOptions;
use security_framework::item::Limit;
use security_framework::item::Location;
use security_framework::item::SearchResult;
use security_framework::item::update_item;
use security_framework::passwords::PasswordOptions;
use security_framework::passwords::delete_generic_password_options;
use security_framework::passwords::generic_password;
use security_framework::passwords::set_generic_password_options;
use security_framework_sys::base::errSecItemNotFound;
use security_framework_sys::item::kSecAttrAccount;
use std::fmt;

#[derive(Debug)]
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

const SHARED_ACCESS_GROUP: &str = "2DC432GLL2.com.openai.shared";
const CODEX_KEYCHAIN_SERVICES: [&str; 3] = ["Codex Auth", "Codex MCP Credentials", "codex"];

pub(crate) fn load_password(
    service: &str,
    account: &str,
) -> Result<Option<String>, MacOsAccessGroupError> {
    if let Some(password) = load_shared_password_bytes(service, account)? {
        return decode_password(password, service, account).map(Some);
    }

    if migrate_password_to_access_group(service, account)?
        && let Some(password) = load_shared_password_bytes(service, account)?
    {
        return decode_password(password, service, account).map(Some);
    }

    load_legacy_password_bytes(service, account)?
        .map(|password| decode_password(password, service, account))
        .transpose()
}

pub(crate) fn save_password(
    service: &str,
    account: &str,
    value: &str,
) -> Result<(), MacOsAccessGroupError> {
    if load_shared_password_bytes(service, account)?.is_none() {
        let _ = migrate_password_to_access_group(service, account)?;
    }

    save_shared_password_bytes(service, account, value.as_bytes())
}

pub(crate) fn delete_password(service: &str, account: &str) -> Result<bool, MacOsAccessGroupError> {
    let shared_removed = delete_shared_password(service, account)?;
    let legacy_removed = delete_legacy_password(service, account)?;
    Ok(shared_removed || legacy_removed)
}

pub(crate) fn migrate_existing_codex_items_to_access_group() -> Result<(), MacOsAccessGroupError> {
    for service in CODEX_KEYCHAIN_SERVICES {
        for account in legacy_accounts_for_service(service)? {
            let _ = migrate_password_to_access_group(service, &account)?;
        }
    }

    Ok(())
}

fn migrate_password_to_access_group(
    service: &str,
    account: &str,
) -> Result<bool, MacOsAccessGroupError> {
    let mut search = ItemSearchOptions::new();
    search
        .class(ItemClass::generic_password())
        .service(service)
        .account(account);

    let mut update = ItemUpdateOptions::new();
    update
        .set_access_group(SHARED_ACCESS_GROUP)
        .set_location(Location::DataProtectionKeychain);

    match update_item(&search, &update) {
        Ok(()) => Ok(true),
        Err(error) if error.code() == errSecItemNotFound => Ok(false),
        Err(error) => Err(wrap_security_error(
            "move existing generic password into the shared access group",
            error,
        )),
    }
}

fn legacy_accounts_for_service(service: &str) -> Result<Vec<String>, MacOsAccessGroupError> {
    let mut search = ItemSearchOptions::new();
    search
        .class(ItemClass::generic_password())
        .service(service)
        .load_attributes(true)
        .limit(Limit::All);

    let results = match search.search() {
        Ok(results) => results,
        Err(error) if error.code() == errSecItemNotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(wrap_security_error(
                "search existing generic passwords for Codex service",
                error,
            ));
        }
    };

    let mut accounts = Vec::with_capacity(results.len());
    for result in results {
        match result {
            SearchResult::Dict(attributes) => accounts.push(account_from_attributes(&attributes)?),
            SearchResult::Ref(_) | SearchResult::Data(_) | SearchResult::Other => {
                return Err(MacOsAccessGroupError::new(
                    "search existing generic passwords for Codex service returned an unexpected result",
                ));
            }
        }
    }

    Ok(accounts)
}

fn account_from_attributes(attributes: &CFDictionary) -> Result<String, MacOsAccessGroupError> {
    let Some(account_value) = attributes.find(unsafe { kSecAttrAccount }.cast()) else {
        return Err(MacOsAccessGroupError::new(
            "generic password attributes did not include an account name",
        ));
    };
    Ok(unsafe { CFString::wrap_under_get_rule((*account_value).cast()) }.to_string())
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

fn load_shared_password_bytes(
    service: &str,
    account: &str,
) -> Result<Option<Vec<u8>>, MacOsAccessGroupError> {
    load_password_bytes(
        shared_password_options(service, account),
        "read shared generic password",
    )
}

fn load_legacy_password_bytes(
    service: &str,
    account: &str,
) -> Result<Option<Vec<u8>>, MacOsAccessGroupError> {
    load_password_bytes(
        legacy_password_options(service, account),
        "read legacy generic password",
    )
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

fn save_shared_password_bytes(
    service: &str,
    account: &str,
    value: &[u8],
) -> Result<(), MacOsAccessGroupError> {
    set_generic_password_options(value, shared_password_options(service, account))
        .map_err(|error| wrap_security_error("write shared generic password", error))
}

fn delete_shared_password(service: &str, account: &str) -> Result<bool, MacOsAccessGroupError> {
    delete_password_options(
        shared_password_options(service, account),
        "delete shared generic password",
    )
}

fn delete_legacy_password(service: &str, account: &str) -> Result<bool, MacOsAccessGroupError> {
    delete_password_options(
        legacy_password_options(service, account),
        "delete legacy generic password",
    )
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

    #[test]
    fn codex_keychain_services_match_stored_secret_services() {
        assert_eq!(
            CODEX_KEYCHAIN_SERVICES,
            ["Codex Auth", "Codex MCP Credentials", "codex"]
        );
    }

    #[test]
    fn shared_access_group_matches_configured_team_prefix() {
        assert_eq!(SHARED_ACCESS_GROUP, "2DC432GLL2.com.openai.shared");
    }
}
