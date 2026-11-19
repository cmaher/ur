//! Machine identity: the `[machine]` section of `ur.toml`.
//!
//! Every host running ur has a short, human-chosen name (`[machine] id`). When
//! the section or key is absent, the id defaults to the host's normalized short
//! hostname. The server runs in a container and cannot see the host's hostname,
//! so `ur start` resolves the id on the host and passes it to the server
//! container as [`UR_MACHINE_ID_ENV`].

use anyhow::Context;
use serde::Deserialize;

/// Env var carrying the host-resolved machine id into the server container.
pub const UR_MACHINE_ID_ENV: &str = "UR_MACHINE_ID";

/// Maximum machine id length (one DNS label).
const MAX_MACHINE_ID_LEN: usize = 63;

/// Resolved `[machine]` section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineConfig {
    /// Machine name, matching `^[a-z0-9][a-z0-9-]{0,62}$`.
    pub id: String,
}

/// Raw TOML representation for the `[machine]` section.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct RawMachineConfig {
    id: Option<String>,
}

/// Resolve the `[machine]` section: an explicit `id` must be valid as written;
/// otherwise the id is derived from the host's hostname.
pub(crate) fn resolve_machine(raw: Option<RawMachineConfig>) -> anyhow::Result<MachineConfig> {
    match raw.and_then(|machine| machine.id) {
        Some(id) => {
            validate_machine_id(&id).context("ur.toml: invalid [machine] id")?;
            Ok(MachineConfig { id })
        }
        None => {
            let hostname = whoami::hostname().context(
                "ur.toml: [machine] id is not set and the hostname could not be read; \
                 set [machine] id in ur.toml",
            )?;
            Ok(MachineConfig {
                id: machine_id_from_hostname(&hostname)?,
            })
        }
    }
}

/// Derive a machine id from a hostname: the text before the first `.`,
/// lowercased, with `_` replaced by `-`. Errors when the result is not a valid id.
pub fn machine_id_from_hostname(hostname: &str) -> anyhow::Result<String> {
    let short = hostname.split('.').next().unwrap_or_default();
    let id = short.to_ascii_lowercase().replace('_', "-");
    validate_machine_id(&id).with_context(|| {
        format!(
            "ur.toml: [machine] id is not set and hostname {hostname:?} does not normalize \
             to a valid machine id (got {id:?}); set [machine] id in ur.toml"
        )
    })?;
    Ok(id)
}

/// Check `id` against `^[a-z0-9][a-z0-9-]{0,62}$`.
pub fn validate_machine_id(id: &str) -> anyhow::Result<()> {
    let mut chars = id.chars();
    let Some(first) = chars.next() else {
        anyhow::bail!("machine id must not be empty");
    };
    let valid_first = first.is_ascii_lowercase() || first.is_ascii_digit();
    let valid_rest = chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !valid_first || !valid_rest || id.len() > MAX_MACHINE_ID_LEN {
        anyhow::bail!(
            "machine id {id:?} must be 1-{MAX_MACHINE_ID_LEN} characters of lowercase \
             letters, digits and '-', starting with a letter or digit"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_id_is_used_verbatim() {
        let raw = RawMachineConfig {
            id: Some("mbp".into()),
        };
        let machine = resolve_machine(Some(raw)).unwrap();
        assert_eq!(machine.id, "mbp");
    }

    #[test]
    fn explicit_invalid_id_is_rejected_naming_the_field() {
        for bad in ["", "MBP", "-mbp", "mb_p", "mb.p", &"a".repeat(64)] {
            let raw = RawMachineConfig {
                id: Some(bad.to_string()),
            };
            let err = format!("{:#}", resolve_machine(Some(raw)).unwrap_err());
            assert!(err.contains("[machine] id"), "{bad:?}: {err}");
        }
    }

    #[test]
    fn explicit_id_at_max_length_is_accepted() {
        validate_machine_id(&"a".repeat(63)).unwrap();
        validate_machine_id("0-lima-ur").unwrap();
    }

    #[test]
    fn missing_section_defaults_to_host_hostname() {
        let expected = machine_id_from_hostname(&whoami::hostname().unwrap()).unwrap();
        assert_eq!(resolve_machine(None).unwrap().id, expected);
        let empty = RawMachineConfig { id: None };
        assert_eq!(resolve_machine(Some(empty)).unwrap().id, expected);
    }

    #[test]
    fn hostname_is_shortened_lowercased_and_underscores_replaced() {
        assert_eq!(machine_id_from_hostname("mbp").unwrap(), "mbp");
        assert_eq!(
            machine_id_from_hostname("Chris_MBP.local").unwrap(),
            "chris-mbp"
        );
        assert_eq!(
            machine_id_from_hostname("lima-ur.internal.example.com").unwrap(),
            "lima-ur"
        );
    }

    #[test]
    fn unusable_hostname_errors_telling_user_to_set_id() {
        for bad in ["", ".local", "_box", "héllo", &"a".repeat(64)] {
            let err = format!("{:#}", machine_id_from_hostname(bad).unwrap_err());
            assert!(err.contains("set [machine] id"), "{bad:?}: {err}");
        }
    }
}
