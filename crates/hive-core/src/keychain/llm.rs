//! Hive-owned generic passwords; never touches Claude Code or Codex credentials.

use crate::llm::{ApiKey, Error, Provider};

const SERVICE: &str = "hive.llm";

pub(crate) fn has_key(provider: Provider) -> Result<bool, Error> {
    native::has_key(SERVICE, provider)
}

pub(crate) fn load_key(provider: Provider) -> Result<Option<ApiKey>, Error> {
    native::load_key(SERVICE, provider)
}

pub(crate) fn save_key(provider: Provider, key: &ApiKey) -> Result<(), Error> {
    native::save_key(SERVICE, provider, key)
}

pub(crate) fn delete_key(provider: Provider) -> Result<(), Error> {
    native::delete_key(SERVICE, provider)
}

#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use security_framework::{
        item::{ItemClass, ItemSearchOptions, Limit},
        passwords::{self, PasswordOptions},
    };

    // Apple's errSecItemNotFound. Other OSStatus codes must remain visible failures.
    const ITEM_NOT_FOUND: i32 = -25300;
    // Reported when `security` could not run or exited without an OSStatus.
    const SECURITY_TOOL_UNAVAILABLE: i32 = -1;

    fn failure(
        provider: Provider,
        operation: &'static str,
        error: security_framework::base::Error,
    ) -> Error {
        Error::Keychain {
            provider,
            operation,
            code: error.code(),
        }
    }

    pub(super) fn has_key(service: &str, provider: Provider) -> Result<bool, Error> {
        let result = ItemSearchOptions::new()
            .class(ItemClass::generic_password())
            .service(service)
            .account(provider.account())
            .load_attributes(true)
            .limit(Limit::Max(1))
            .search();
        match result {
            Ok(items) => Ok(!items.is_empty()),
            Err(error) if error.code() == ITEM_NOT_FOUND => Ok(false),
            Err(error) => Err(failure(provider, "check", error)),
        }
    }

    pub(super) fn load_key(service: &str, provider: Provider) -> Result<Option<ApiKey>, Error> {
        let options = PasswordOptions::new_generic_password(service, provider.account());
        let bytes = match passwords::generic_password(options) {
            Ok(bytes) => bytes,
            Err(error) if error.code() == ITEM_NOT_FOUND => return Ok(None),
            Err(error) => return Err(failure(provider, "read", error)),
        };
        let value = String::from_utf8(bytes).map_err(|_| Error::InvalidStoredKey { provider })?;
        ApiKey::try_from(value)
            .map(Some)
            .map_err(|_| Error::InvalidStoredKey { provider })
    }

    /// Writes the item with an ACL that lets any application read it without a prompt.
    ///
    /// The default ACL trusts the writing app by its code identity, which for an ad-hoc signed
    /// build is its `cdhash`: every build is a stranger and macOS asks for the login password
    /// after each update. `security` is the only public API for that ACL (`-A`); the command
    /// goes through stdin so the key never appears in process arguments, and `-X` hex data
    /// needs no quoting. The item ends up in the login keychain like before.
    pub(super) fn save_key(service: &str, provider: Provider, key: &ApiKey) -> Result<(), Error> {
        let hex: String = key.as_str().bytes().map(|b| format!("{b:02x}")).collect();
        let command = format!(
            "add-generic-password -A -U -s {service} -a {} -X {hex}\n",
            provider.account()
        );
        let output = security_tool(&command).map_err(|_| Error::Keychain {
            provider,
            operation: "save",
            code: SECURITY_TOOL_UNAVAILABLE,
        })?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(Error::Keychain {
            provider,
            operation: "save",
            code: returned_status(&stderr)
                .or(output.status.code())
                .unwrap_or(SECURITY_TOOL_UNAVAILABLE),
        })
    }

    /// Runs one command in `security -i`, where the command line is read from stdin.
    fn security_tool(command: &str) -> std::io::Result<std::process::Output> {
        use std::io::Write as _;
        use std::process::{Command, Stdio};
        let mut child = Command::new("security")
            .arg("-i")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(command.as_bytes())?;
        }
        child.wait_with_output()
    }

    /// The OSStatus `security` reports as `<command>: returned <code>` on failure.
    fn returned_status(stderr: &str) -> Option<i32> {
        stderr
            .lines()
            .rev()
            .find_map(|line| line.rsplit_once(": returned ")?.1.trim().parse().ok())
    }

    pub(super) fn delete_key(service: &str, provider: Provider) -> Result<(), Error> {
        match passwords::delete_generic_password(service, provider.account()) {
            Ok(()) => Ok(()),
            Err(error) if error.code() == ITEM_NOT_FOUND => Ok(()),
            Err(error) => Err(failure(provider, "delete", error)),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        #[ignore = "requires an unlocked macOS Keychain; uses only a unique test service"]
        fn keychain_round_trip() {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default();
            let service = format!("hive.llm.test.{}.{nanos}", std::process::id());
            let provider = Provider::OpenAi;
            struct Cleanup(String);
            impl Drop for Cleanup {
                fn drop(&mut self) {
                    let _ = native::delete_key(&self.0, Provider::OpenAi);
                }
            }
            let _cleanup = Cleanup(service.clone());
            assert!(!has_key(&service, provider).unwrap());
            assert!(load_key(&service, provider).unwrap().is_none());
            for value in ["hive-test-first", "hive-test-replacement"] {
                let key = ApiKey::try_from(value.to_owned()).unwrap();
                save_key(&service, provider, &key).unwrap();
                assert!(has_key(&service, provider).unwrap());
                assert_eq!(
                    load_key(&service, provider).unwrap().unwrap().as_str(),
                    value
                );
                assert!(!has_key(&service, Provider::Anthropic).unwrap());
            }
            assert!(
                any_application_may_read(&service, provider),
                "ACL must not depend on this build's code identity"
            );
            delete_key(&service, provider).unwrap();
            delete_key(&service, provider).unwrap();
            assert!(!has_key(&service, provider).unwrap());
            assert!(load_key(&service, provider).unwrap().is_none());
        }

        #[test]
        fn returned_status_parses_last_report() {
            let stderr = "security: SecKeychainItemCreateFromContent (<default>): exists.\n\
                          add-generic-password: returned -25299\n";
            assert_eq!(returned_status(stderr), Some(-25299));
            assert_eq!(returned_status("Usage: add-generic-password\n"), None);
        }

        /// The decrypt ACL entry of the item has no application list, which `security`
        /// dumps as `applications: <null>`: any app reads it without a prompt.
        fn any_application_may_read(service: &str, provider: Provider) -> bool {
            let output = std::process::Command::new("security")
                .args(["dump-keychain", "-a"])
                .output()
                .unwrap();
            let dump = String::from_utf8_lossy(&output.stdout);
            let needle = format!("\"svce\"<blob>=\"{service}\"");
            let item = dump.split("keychain:").find(|item| {
                item.contains(&needle)
                    && item.contains(&format!("\"acct\"<blob>=\"{}\"", provider.account()))
            });
            item.is_some_and(|item| {
                item.split("entry ").any(|entry| {
                    entry.contains("decrypt") && entry.contains("applications: <null>")
                })
            })
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod native {
    use super::*;

    pub(super) fn has_key(_: &str, provider: Provider) -> Result<bool, Error> {
        Err(Error::UnsupportedPlatform {
            provider,
            operation: "check",
        })
    }

    pub(super) fn load_key(_: &str, provider: Provider) -> Result<Option<ApiKey>, Error> {
        Err(Error::UnsupportedPlatform {
            provider,
            operation: "read",
        })
    }

    pub(super) fn save_key(_: &str, provider: Provider, _: &ApiKey) -> Result<(), Error> {
        Err(Error::UnsupportedPlatform {
            provider,
            operation: "save",
        })
    }

    pub(super) fn delete_key(_: &str, provider: Provider) -> Result<(), Error> {
        Err(Error::UnsupportedPlatform {
            provider,
            operation: "delete",
        })
    }
}
