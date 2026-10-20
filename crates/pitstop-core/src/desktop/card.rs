//! A Code Local session card: the JSON file Claude Desktop keeps per session
//! in `claude-code-sessions/<account>/<organization>/local_*.json`.
//!
//! A card is read strictly (a duplicate key is refused, not settled) and
//! only fields recognised as the account or organization are rewritten:
//! never a search-and-replace over the text. A field this module does not
//! know that still carries an old id refuses the card instead of guessing.

use std::fmt;
use std::path::Path;

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};

use super::{refuse, Code};
use crate::error::Result;

const MAX_CARD: u64 = 32 * 1024 * 1024;
const ACCOUNT_KEYS: [&str; 4] = ["accountId", "account_id", "accountUuid", "account_uuid"];
const ORG_KEYS: [&str; 6] = [
    "organizationId",
    "organization_id",
    "organizationUuid",
    "organization_uuid",
    "orgId",
    "org_id",
];

/// A JSON value whose objects may not repeat a key.
struct Strict(Value);

impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        d.deserialize_any(StrictVisitor).map(Strict)
    }
}

struct StrictVisitor;

impl<'de> Visitor<'de> for StrictVisitor {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("JSON")
    }
    fn visit_bool<E>(self, v: bool) -> std::result::Result<Value, E> {
        Ok(Value::Bool(v))
    }
    fn visit_i64<E>(self, v: i64) -> std::result::Result<Value, E> {
        Ok(v.into())
    }
    fn visit_u64<E>(self, v: u64) -> std::result::Result<Value, E> {
        Ok(v.into())
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<Value, E> {
        serde_json::Number::from_f64(v)
            .map(Value::Number)
            .ok_or_else(|| E::custom("not a finite number"))
    }
    fn visit_str<E>(self, v: &str) -> std::result::Result<Value, E> {
        Ok(Value::String(v.to_owned()))
    }
    fn visit_string<E>(self, v: String) -> std::result::Result<Value, E> {
        Ok(Value::String(v))
    }
    fn visit_unit<E>(self) -> std::result::Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_none<E>(self) -> std::result::Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Value, A::Error> {
        let mut items = Vec::new();
        while let Some(Strict(item)) = seq.next_element()? {
            items.push(item);
        }
        Ok(Value::Array(items))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<Value, A::Error> {
        let mut object = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if object.contains_key(&key) {
                return Err(de::Error::custom(format!("duplicate key `{key}`")));
            }
            let Strict(value) = map.next_value()?;
            object.insert(key, value);
        }
        Ok(Value::Object(object))
    }
}

/// Parse JSON text, refusing a repeated key. A UTF-8 byte order mark is
/// allowed, as Python's `utf-8-sig` allows it.
pub fn parse(bytes: &[u8]) -> Result<Value> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    serde_json::from_slice::<Strict>(bytes)
        .map(|Strict(v)| v)
        .map_err(|e| {
            refuse(
                Code::InvalidCard,
                format!("JSON that cannot be used as is: {e}"),
            )
        })
}

/// Read a JSON file: never through a symlink, never a file over 32 MB.
pub fn read(path: &Path) -> Result<Value> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| super::io(path, e))?;
    if !meta.is_file() {
        return Err(refuse(
            Code::Unsafe,
            format!(
                "refusing to read JSON through a symlink or non-file: {}",
                path.display()
            ),
        ));
    }
    if meta.len() > MAX_CARD {
        return Err(refuse(
            Code::InvalidCard,
            format!("card over 32 MB: {}", path.display()),
        ));
    }
    parse(&std::fs::read(path).map_err(|e| super::io(path, e))?)
}

/// JSON as the cards are written: two-space indent, a final newline.
pub fn to_bytes(value: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(value).expect("a JSON value always serializes");
    bytes.push(b'\n');
    bytes
}

pub(super) fn is_session_id(s: &str) -> bool {
    (1..=200).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The card's `cliSessionId`, wherever it sits; every copy must agree.
pub fn session_id(card: &Value) -> Result<Option<String>> {
    fn walk<'a>(node: &'a Value, found: &mut Vec<&'a Value>) {
        match node {
            Value::Object(map) => {
                for (key, item) in map {
                    if key == "cliSessionId" {
                        found.push(item);
                    } else {
                        walk(item, found);
                    }
                }
            }
            Value::Array(items) => items.iter().for_each(|i| walk(i, found)),
            _ => {}
        }
    }
    let mut found = Vec::new();
    walk(card, &mut found);
    let Some(first) = found.first() else {
        return Ok(None);
    };
    let ids: Vec<&str> = found.iter().filter_map(|v| v.as_str()).collect();
    if ids.len() != found.len() || !ids.iter().all(|s| is_session_id(s)) {
        return Err(refuse(Code::InvalidCard, "cliSessionId is not valid"));
    }
    if ids.iter().any(|s| *s != ids[0]) {
        return Err(refuse(
            Code::InvalidCard,
            "a card holds several different cliSessionIds",
        ));
    }
    Ok(first.as_str().map(str::to_owned))
}

/// The card as the target account would have written it: recognised
/// account and organization fields moved from `from` to `to`
/// (`account/organization` each); everything else as it was.
pub fn transform(card: &Value, from: &str, to: &str) -> Result<Value> {
    let (old_account, old_org) = from.split_once('/').expect("a pair has two halves");
    let (new_account, new_org) = to.split_once('/').expect("a pair has two halves");
    let mut unmapped: Vec<String> = Vec::new();

    fn walk(
        node: &Value,
        key: &str,
        parent: &str,
        ids: (&str, &str, &str, &str),
        unmapped: &mut Vec<String>,
    ) -> Result<Value> {
        let (old_account, old_org, new_account, new_org) = ids;
        match node {
            Value::Object(map) => {
                let mut out = Map::new();
                for (name, item) in map {
                    out.insert(name.clone(), walk(item, name, key, ids, unmapped)?);
                }
                Ok(Value::Object(out))
            }
            Value::Array(items) => items
                .iter()
                .map(|item| walk(item, key, parent, ids, unmapped))
                .collect::<Result<Vec<_>>>()
                .map(Value::Array),
            Value::String(s) => {
                let account_key = ACCOUNT_KEYS.contains(&key)
                    || (matches!(key, "id" | "uuid") && parent == "account");
                let org_key = ORG_KEYS.contains(&key)
                    || (matches!(key, "id" | "uuid") && matches!(parent, "organization" | "org"));
                if account_key {
                    if s == old_account {
                        return Ok(Value::String(new_account.to_owned()));
                    }
                    if !s.is_empty() && s != new_account {
                        return Err(refuse(
                            Code::InvalidCard,
                            "an account field does not match the source folder",
                        ));
                    }
                }
                if org_key {
                    if s == old_org {
                        return Ok(Value::String(new_org.to_owned()));
                    }
                    if !s.is_empty() && s != new_org {
                        return Err(refuse(
                            Code::InvalidCard,
                            "an organization field does not match the source folder",
                        ));
                    }
                }
                for (old, new) in [(old_account, new_account), (old_org, new_org)] {
                    if old != new && s.contains(old) {
                        unmapped.push(key.to_owned());
                    }
                }
                Ok(node.clone())
            }
            _ => Ok(node.clone()),
        }
    }

    let out = walk(
        card,
        "",
        "",
        (old_account, old_org, new_account, new_org),
        &mut unmapped,
    )?;
    if !unmapped.is_empty() {
        unmapped.sort();
        unmapped.dedup();
        return Err(refuse(
            Code::InvalidCard,
            format!(
                "account or organization id in unsupported fields: {}",
                unmapped.join(", ")
            ),
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_repeated_key_is_refused_and_a_bom_is_allowed() {
        assert!(parse(br#"{"a":1,"a":2}"#).is_err());
        assert!(parse(br#"{"a":{"b":1,"b":1}}"#).is_err());
        assert_eq!(parse(b"\xEF\xBB\xBF{\"a\":1}").unwrap(), json!({"a": 1}));
    }

    #[test]
    fn the_session_id_is_found_anywhere_and_must_agree() {
        assert_eq!(
            session_id(&json!({"x": {"cliSessionId": "abc-1"}}))
                .unwrap()
                .as_deref(),
            Some("abc-1")
        );
        assert_eq!(session_id(&json!({"title": "t"})).unwrap(), None);
        assert!(session_id(&json!({"cliSessionId": "a", "y": [{"cliSessionId": "b"}]})).is_err());
        assert!(session_id(&json!({"cliSessionId": "../etc"})).is_err());
        assert!(session_id(&json!({"cliSessionId": 7})).is_err());
    }

    #[test]
    fn only_recognised_identity_fields_move_to_the_target() {
        let card = json!({
            "title": "Fix the build",
            "cliSessionId": "s1",
            "accountUuid": "aaa",
            "organizationUuid": "ooo",
            "account": {"uuid": "aaa", "email": "x@y"},
            "organization": {"id": "ooo"},
            "list": [{"orgId": "ooo"}],
        });
        let out = transform(&card, "aaa/ooo", "bbb/ppp").unwrap();
        assert_eq!(out["accountUuid"], "bbb");
        assert_eq!(out["organizationUuid"], "ppp");
        assert_eq!(out["account"]["uuid"], "bbb");
        assert_eq!(out["organization"]["id"], "ppp");
        assert_eq!(out["list"][0]["orgId"], "ppp");
        assert_eq!(out["title"], "Fix the build");
        assert_eq!(out["cliSessionId"], "s1");
    }

    #[test]
    fn an_old_id_left_in_an_unknown_field_refuses_the_card() {
        let card = json!({"accountUuid": "aaa", "link": "https://x/aaa/chat"});
        assert!(transform(&card, "aaa/ooo", "bbb/ppp").is_err());
    }

    #[test]
    fn an_identity_field_of_a_third_account_refuses_the_card() {
        let card = json!({"accountUuid": "zzz"});
        assert!(transform(&card, "aaa/ooo", "bbb/ppp").is_err());
        // Already the target's: kept.
        let card = json!({"accountUuid": "bbb"});
        assert_eq!(transform(&card, "aaa/ooo", "bbb/ppp").unwrap(), card);
    }
}
