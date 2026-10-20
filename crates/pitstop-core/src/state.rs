use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Error, Result};
use crate::host::Host;
use crate::provider::ProviderId;

/// Non-secret metadata only. Credentials never go in here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountMeta {
    pub name: String,
    pub provider: ProviderId,
    pub label: Option<String>,
    pub parked_at: Option<u64>,
    /// Who the parked login belongs to (see `provider::Details`). Absent in
    /// state written by 0.1.0 until the account is next parked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    pub accounts: Vec<AccountMeta>,
    /// provider id -> name of the account currently holding the live credential.
    pub active: BTreeMap<String, String>,
    /// Accounts of tools no longer switched here (Claude Code), kept as they
    /// were written so nothing is lost, and written back untouched.
    pub(crate) retired: Vec<Value>,
}

/// `state.json` as it is on disk.
#[derive(Serialize, Deserialize)]
struct OnDisk {
    #[serde(default)]
    accounts: Vec<Value>,
    #[serde(default)]
    active: BTreeMap<String, String>,
}

pub fn state_path(host: &dyn Host) -> PathBuf {
    host.home().join(".pitstop").join("state.json")
}

impl State {
    pub fn load(host: &dyn Host) -> Result<State> {
        match host.read(&state_path(host))? {
            None => Ok(State::default()),
            Some(bytes) => {
                let bad = |e: serde_json::Error| Error::InvalidData(format!("state.json: {e}"));
                let disk: OnDisk = serde_json::from_slice(&bytes).map_err(bad)?;
                let mut state = State {
                    active: disk.active,
                    ..State::default()
                };
                for account in disk.accounts {
                    let known = account
                        .get("provider")
                        .and_then(Value::as_str)
                        .is_some_and(|p| ProviderId::parse(p).is_ok());
                    if known {
                        state
                            .accounts
                            .push(serde_json::from_value(account).map_err(bad)?);
                    } else {
                        state.retired.push(account);
                    }
                }
                Ok(state)
            }
        }
    }

    pub fn save(&self, host: &dyn Host) -> Result<()> {
        let bad = |e: serde_json::Error| Error::InvalidData(format!("state.json: {e}"));
        let mut accounts = self
            .accounts
            .iter()
            .map(serde_json::to_value)
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(bad)?;
        accounts.extend(self.retired.iter().cloned());
        let disk = OnDisk {
            accounts,
            active: self.active.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&disk).map_err(bad)?;
        host.write_atomic(&state_path(host), &bytes)
    }

    pub fn active_name(&self, provider: ProviderId) -> Option<&String> {
        self.active.get(provider.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeHost;

    #[test]
    fn load_missing_state_is_empty() {
        let host = FakeHost::new();
        let state = State::load(&host).unwrap();
        assert!(state.accounts.is_empty());
        assert!(state.active.is_empty());
    }

    #[test]
    fn save_then_load_roundtrips() {
        let host = FakeHost::new();
        let mut state = State::default();
        state.accounts.push(AccountMeta {
            name: "work".into(),
            provider: ProviderId::Codex,
            label: Some("user@example.com".into()),
            parked_at: Some(1_760_000_000),
            identity: Some("acc-1:org-1".into()),
            plan: Some("Max 20x".into()),
        });
        state.active.insert("codex".into(), "work".into());
        state.save(&host).unwrap();
        assert_eq!(State::load(&host).unwrap(), state);
    }

    #[test]
    fn state_written_by_0_1_0_still_loads() {
        let host = FakeHost::new();
        host.put_file(
            state_path(&host),
            r#"{"accounts":[{"name":"work","provider":"codex","label":"a@x.com","parked_at":1760000000}],"active":{"codex":"work"}}"#,
        );
        let state = State::load(&host).unwrap();
        assert_eq!(state.accounts[0].identity, None);
        assert_eq!(state.accounts[0].plan, None);
        assert_eq!(state.active_name(ProviderId::Codex).unwrap(), "work");
    }

    #[test]
    fn claude_code_accounts_are_left_out_but_kept_on_disk() {
        let host = FakeHost::new();
        let claude = r#"{"name":"old","provider":"claude-code","label":"a@x.com","parked_at":1760000000,"identity":"u:o"}"#;
        host.put_file(
            state_path(&host),
            format!(
                r#"{{"accounts":[{claude},{{"name":"work","provider":"codex","label":"b@x.com","parked_at":1}}],"active":{{"claude-code":"old","codex":"work"}}}}"#
            ),
        );
        let mut state = State::load(&host).unwrap();
        let names: Vec<_> = state.accounts.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["work"]);
        state.accounts[0].label = Some("c@x.com".into());
        state.save(&host).unwrap();
        let disk: Value = serde_json::from_slice(&host.file(&state_path(&host)).unwrap()).unwrap();
        assert_eq!(
            disk["accounts"][1],
            serde_json::from_str::<Value>(claude).unwrap()
        );
        assert_eq!(disk["active"]["claude-code"], "old");
        assert_eq!(State::load(&host).unwrap(), state);
    }
}
