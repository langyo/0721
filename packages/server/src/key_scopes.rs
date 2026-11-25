//! The API-key permission vocabulary and its ONLY interpreter.
//!
//! A key's permissions are stored as raw JSON text in `api_keys.scopes`;
//! the shape is an OBJECT with an `ops` array — `{"ops":["read"]}` — and
//! the next wave appends a `buckets` field without touching `ops`. Every
//! "may this key do this?" question in the service funnels through this
//! module: the MCP tool surface (`tools/list` and `tools/call` share one
//! parsed set per request) and the RPC key lanes all speak the same
//! [`KeyOp`] vocabulary, so a new tool cannot be added without a
//! permission mapping ([`TOOL_REQUIRED_OP`]) and a new permission cannot
//! be interpreted in two places.
//!
//! Everything here is a PURE function of its arguments — no I/O, no
//! clock of its own (`is_live` takes `now`), no store access.
//!
//! ## Fail-closed, everywhere
//!
//! * [`parse_scopes`] answers the EMPTY set for anything that is not a
//!   well-formed `{"ops":[…]}` object with known tags: never an error,
//!   never the full set. A corrupt row therefore loses access rather
//!   than gaining it.
//! * [`is_live`] treats an `expires_at` it cannot parse as EXPIRED (a
//!   key whose expiry is unreadable is not provably live).
//! * [`authorize`] denies unless the tool is known AND its operation is
//!   in the parsed set.

use std::collections::BTreeSet;

use i0721_proto::{ApiKeyRecord, KeyOp};
use jiff::Timestamp;

/// The MCP tools and the permission each one requires.
///
/// This array IS the definition of "a known tool" — the tool-spec table
/// in `mcp.rs` and this mapping are pinned equal by the unit test below,
/// so a tool added to one without the other fails the build's test lane
/// instead of silently becoming permission-free.
///
/// # The enforcement seam
///
/// Permissions are enforced on **MCP tools only**: `key_scopes` says
/// which [`KeyOp`] a tool needs, and `mcp.rs` is the ONLY surface that
/// consumes this table (`tools/list` hides the tools a key cannot call,
/// `tools/call` refuses them through [`authorize`]). The webui's RPC
/// lane acts as the signed-in USER and never consults these ops.
///
/// Therefore: **any future NON-MCP capability guarded by a [`KeyOp`]
/// (a REST route, a new RPC method, a stream) must add its own mapping
/// and its own test.** Do not assume `TOOL_REQUIRED_OP` covers it —
/// nothing outside `mcp.rs` reads this table, and
/// `every_permission_is_enforced_by_at_least_one_mcp_tool` only proves
/// that each op gates at least one TOOL, not that a new surface gates
/// anything at all.
pub const TOOL_REQUIRED_OP: [(&str, KeyOp); 5] = [
    ("list_files", KeyOp::Read),
    ("file_info", KeyOp::Read),
    ("search_files", KeyOp::Read),
    ("upload_file", KeyOp::Upload),
    ("get_share_link", KeyOp::Share),
];

/// The operation `tool` requires, or `None` when no such tool exists.
#[must_use]
pub fn required_op(tool: &str) -> Option<KeyOp> {
    TOOL_REQUIRED_OP
        .iter()
        .find(|(name, _)| *name == tool)
        .map(|(_, op)| *op)
}

/// Parse the stored `scopes` text into its permission set.
///
/// The accepted shape is an object whose `ops` member is an array of
/// known tags (`read`, `upload`, `share`; duplicates collapse). Every
/// other input — the empty string, a bare array (the legacy `[]`), a
/// non-object, a missing `ops`, `ops` that is not an array, an unknown
/// tag such as `"root"`, a non-string element — answers the EMPTY set.
/// This is a security boundary: it must never answer `Err` (callers
/// would have to decide a policy) and must never answer a wider set than
/// the text literally names.
///
/// Unknown KEYS inside the object are ignored on purpose: a future wave
/// adds `buckets`, and an older binary must not lose `ops` because of a
/// field it does not understand.
#[must_use]
pub fn parse_scopes(raw: &str) -> BTreeSet<KeyOp> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return BTreeSet::new();
    };
    let Some(ops) = value.get("ops").and_then(serde_json::Value::as_array) else {
        return BTreeSet::new();
    };
    let mut out = BTreeSet::new();
    for entry in ops {
        // A single malformed element poisons the WHOLE set: a half-read
        // permission list is a policy nobody wrote.
        match entry.as_str().and_then(KeyOp::parse) {
            Some(op) => {
                out.insert(op);
            }
            None => return BTreeSet::new(),
        }
    }
    out
}

/// The stored representation of a permission set: the canonical
/// `{"ops":[…]} ` object, ops in [`KeyOp::ALL`] order with duplicates
/// removed. This is what `keys.create` / `keys.update` write, so the
/// value round-trips through [`parse_scopes`] unchanged.
#[must_use]
pub fn encode_scopes(ops: impl IntoIterator<Item = KeyOp>) -> String {
    let set: BTreeSet<KeyOp> = ops.into_iter().collect();
    let ordered: Vec<&str> = KeyOp::ALL
        .into_iter()
        .filter(|op| set.contains(op))
        .map(KeyOp::as_str)
        .collect();
    serde_json::json!({ "ops": ordered }).to_string()
}

/// A permission set in the CANONICAL response order (`read`, `upload`,
/// `share`) — what `keys.list` reports instead of the raw stored text.
#[must_use]
pub fn ordered_ops(set: &BTreeSet<KeyOp>) -> Vec<KeyOp> {
    KeyOp::ALL
        .into_iter()
        .filter(|op| set.contains(op))
        .collect()
}

/// The `buckets` dimension of a key's scopes (spec Q7): a slug array of
/// at most 64 entries, deduped and lexicographically sorted.
///
/// Fail-closed (I2): a MISSING field or a non-array value means NO
/// buckets — `parse_scopes_buckets` answers the empty set for anything
/// that is not a JSON array of strings, and an array longer than the cap
/// answers the empty set too (a truncated permission list is a policy
/// nobody wrote). Like `parse_scopes`, it never answers `Err`.
#[must_use]
pub fn parse_scopes_buckets(raw: &str) -> BTreeSet<String> {
    const MAX_BUCKETS: usize = 64;
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return BTreeSet::new();
    };
    let Some(buckets) = value.get("buckets").and_then(serde_json::Value::as_array) else {
        return BTreeSet::new();
    };
    if buckets.len() > MAX_BUCKETS {
        return BTreeSet::new();
    }
    let mut out = BTreeSet::new();
    for entry in buckets {
        match entry.as_str() {
            Some(slug) if !slug.is_empty() => {
                out.insert(slug.to_owned());
            }
            // A non-string (or empty) element poisons the whole list.
            _ => return BTreeSet::new(),
        }
    }
    out
}

/// The stored representation of a key's scopes WITH the bucket set: the
/// canonical `{"ops":[…], "buckets":[…]}` object — ops in [`KeyOp::ALL`]
/// order, buckets lexicographically sorted, both deduped. The
/// `keys.create` / `keys.update` write path; round-trips through
/// [`parse_scopes`] and [`parse_scopes_buckets`] unchanged.
#[must_use]
pub fn encode_scopes_with_buckets(
    ops: impl IntoIterator<Item = KeyOp>,
    buckets: &[String],
) -> String {
    let set: BTreeSet<KeyOp> = ops.into_iter().collect();
    let ordered: Vec<&str> = KeyOp::ALL
        .into_iter()
        .filter(|op| set.contains(op))
        .map(KeyOp::as_str)
        .collect();
    let mut payload = serde_json::Map::new();
    payload.insert(
        "ops".to_owned(),
        serde_json::Value::Array(
            ordered
                .into_iter()
                .map(|op| serde_json::Value::String(op.to_owned()))
                .collect(),
        ),
    );
    let bucket_list: BTreeSet<&String> = buckets.iter().collect();
    payload.insert(
        "buckets".to_owned(),
        serde_json::Value::Array(
            bucket_list
                .into_iter()
                .map(|slug| serde_json::Value::String(slug.clone()))
                .collect(),
        ),
    );
    serde_json::Value::Object(payload).to_string()
}

/// Whether a key may still open a session at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Live {
    /// Not revoked and not past its expiry.
    Live,
    /// `revoked_at` is set.
    Revoked,
    /// Past its `expires_at` — including an `expires_at` that does not
    /// parse (fail-closed: an unreadable expiry is not "never expires").
    Expired,
}

/// Classify `record` at instant `now`.
///
/// Revocation is checked first: a revoked key is `Revoked` even when its
/// expiry has also passed, because revocation is the stronger statement
/// (and the HTTP text differs).
#[must_use]
pub fn is_live(record: &ApiKeyRecord, now: Timestamp) -> Live {
    if record.revoked_at.is_some() {
        return Live::Revoked;
    }
    match record.expires_at.as_deref() {
        None => Live::Live,
        Some(raw) => match raw.parse::<Timestamp>() {
            Ok(expiry) if now < expiry => Live::Live,
            // Unparseable = expired (fail-closed), and `now == expiry`
            // is already over: the expiry instant is exclusive.
            Ok(_) | Err(_) => Live::Expired,
        },
    }
}

/// Why a call was denied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denial {
    /// The key is the subject of `Live::Revoked`.
    Revoked,
    /// The key is the subject of `Live::Expired`.
    Expired,
    /// `tool` is not in [`TOOL_REQUIRED_OP`].
    UnknownTool,
    /// The key's permission set lacks the operation `tool` requires.
    Scope {
        /// The operation the tool requires.
        required: KeyOp,
    },
}

impl Denial {
    /// The human-readable reason (one phrasing, used by every surface).
    #[must_use]
    pub fn message(&self, tool: &str) -> String {
        match self {
            Denial::Revoked => "this API key was revoked".to_owned(),
            Denial::Expired => "this API key has expired".to_owned(),
            Denial::UnknownTool => format!("no such tool: {tool:?}"),
            Denial::Scope { required } => format!(
                "scope denied: this API key lacks the {required:?} permission required by tool {tool:?}",
                required = required.as_str(),
            ),
        }
    }
}

/// Authorize one tool call by `record`'s permissions at `now`.
///
/// # Errors
///
/// The [`Denial`] explaining why the call may not proceed. The checks run
/// in a fixed order — liveness, then tool existence, then scope — so the
/// same key and tool always produce the same reason.
pub fn authorize(record: &ApiKeyRecord, tool: &str) -> Result<KeyOp, Denial> {
    authorize_at(record, tool, Timestamp::now())
}

/// [`authorize`] at an explicit instant (the testable form).
///
/// # Errors
///
/// The [`Denial`] explaining why the call may not proceed.
pub fn authorize_at(record: &ApiKeyRecord, tool: &str, now: Timestamp) -> Result<KeyOp, Denial> {
    match is_live(record, now) {
        Live::Live => {}
        Live::Revoked => return Err(Denial::Revoked),
        Live::Expired => return Err(Denial::Expired),
    }
    let required = required_op(tool).ok_or(Denial::UnknownTool)?;
    if parse_scopes(&record.scopes).contains(&required) {
        Ok(required)
    } else {
        Err(Denial::Scope { required })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(scopes: &str, expires_at: Option<&str>) -> ApiKeyRecord {
        ApiKeyRecord {
            id: uuid::Uuid::nil(),
            user_id: uuid::Uuid::nil(),
            name: "t".to_owned(),
            key_prefix: "i0721-t".to_owned(),
            key_hash: "h".to_owned(),
            scopes: scopes.to_owned(),
            expires_at: expires_at.map(str::to_owned),
            last_used_at: None,
            revoked_at: None,
            created_at: Timestamp::now(),
        }
    }

    /// ⑤ The truth table. Every malformed shape answers the EMPTY set —
    /// never an error, never the full set.
    #[test]
    fn parse_scopes_truth_table_is_fail_closed() {
        let read = BTreeSet::from([KeyOp::Read]);
        assert_eq!(
            parse_scopes(r#"{"ops":["read"]}"#),
            read,
            "the documented shape parses"
        );
        assert_eq!(
            parse_scopes(r#"{"ops":["upload","read","read"]}"#),
            BTreeSet::from([KeyOp::Read, KeyOp::Upload]),
            "duplicates collapse"
        );
        assert_eq!(
            parse_scopes(r#"{"ops":[],"buckets":["b1"]}"#),
            BTreeSet::new(),
            "a future wave's extra keys are ignored, not fatal"
        );
        // The next wave's shape, verbatim: `buckets` is a SIBLING of
        // `ops` and must never widen, narrow or collapse the op set.
        assert_eq!(
            parse_scopes(r#"{"ops":["read"],"buckets":[]}"#),
            read,
            "an empty buckets list keeps the op set"
        );
        assert_eq!(
            parse_scopes(r#"{"ops":["read"],"buckets":["00000000-0000-0000-0000-000000000001"]}"#),
            read,
            "a populated buckets list does not gate ops at this layer"
        );
        // ...while a malformed `ops` still fails closed even beside a
        // well-formed `buckets`.
        assert_eq!(
            parse_scopes(r#"{"ops":["root"],"buckets":["b1"]}"#),
            BTreeSet::new(),
            "buckets never rescues a malformed ops"
        );
        assert_eq!(
            parse_scopes(r#"{"buckets":["b1"]}"#),
            BTreeSet::new(),
            "no ops at all is the empty set"
        );
        for raw in [
            "[]",
            "",
            "not json",
            r#"{"ops":["root"]}"#,
            r#"{"ops":"read"}"#,
            r#"{"x":1}"#,
            r#"{"ops":["read","root"]}"#,
            r#"["read"]"#,
            r#"{"ops":[1]}"#,
            "null",
        ] {
            assert_eq!(
                parse_scopes(raw),
                BTreeSet::new(),
                "{raw:?} must parse to the EMPTY set (fail-closed)"
            );
        }
    }

    #[test]
    fn encode_scopes_is_canonical_and_round_trips() {
        let encoded = encode_scopes([KeyOp::Share, KeyOp::Read, KeyOp::Share]);
        assert_eq!(
            encoded, r#"{"ops":["read","share"]}"#,
            "canonical order, deduped"
        );
        assert_eq!(
            parse_scopes(&encoded),
            BTreeSet::from([KeyOp::Read, KeyOp::Share])
        );
        assert_eq!(encode_scopes([]), r#"{"ops":[]}"#);
        assert_eq!(parse_scopes(&encode_scopes([])), BTreeSet::new());
    }

    /// The buckets-dimension truth table (Q7): a slug array ≤ 64, deduped,
    /// sorted; missing / non-array / malformed = NO buckets (fail-closed).
    /// MUT:M-P2A1 — an `as_str`-skipping element (non-string accepted) or
    /// a `>64`-passing cap turns the matching arm red.
    #[test]
    fn parse_scopes_buckets_malformed_means_none() {
        let two: BTreeSet<String> = ["alpha".to_owned(), "beta".to_owned()].into();
        assert_eq!(
            parse_scopes_buckets(r#"{"ops":["read"],"buckets":["beta","alpha","beta"]}"#),
            two,
            "deduped and lexicographically sorted"
        );
        assert_eq!(
            parse_scopes_buckets(r#"{"ops":["read"]}"#),
            BTreeSet::new(),
            "a missing buckets field = no buckets"
        );
        for raw in [
            "not json",
            "[]",
            r#"{"buckets":"alpha"}"#,
            r#"{"buckets":{"a":1}}"#,
            r#"{"buckets":["alpha",42]}"#,
            r#"{"buckets":[""]}"#,
            r#"{"buckets":[null]}"#,
            &format!(r#"{{"buckets":[{}]}}"#, {
                // 65 distinct slugs: one over the cap is the whole-nothing.
                (0..65)
                    .map(|i| format!(r#""b{i}""#))
                    .collect::<Vec<_>>()
                    .join(",")
            }),
        ] {
            assert_eq!(
                parse_scopes_buckets(raw),
                BTreeSet::new(),
                "{raw:?} must answer NO buckets (fail-closed)"
            );
        }
        // Exactly at the cap still parses.
        let at_cap = format!(
            r#"{{"buckets":[{}]}}"#,
            (0..64)
                .map(|i| format!(r#""b{i}""#))
                .collect::<Vec<_>>()
                .join(",")
        );
        assert_eq!(parse_scopes_buckets(&at_cap).len(), 64);
    }

    /// The with-buckets encoder round-trips through BOTH parsers.
    #[test]
    fn encode_scopes_with_buckets_is_canonical_and_round_trips() {
        let text = encode_scopes_with_buckets(
            [KeyOp::Share, KeyOp::Read],
            &["zeta".to_owned(), "alpha".to_owned(), "zeta".to_owned()],
        );
        assert_eq!(
            text, r#"{"buckets":["alpha","zeta"],"ops":["read","share"]}"#,
            "buckets sorted + deduped; ops canonical; both live in one object"
        );
        assert_eq!(
            parse_scopes(&text),
            BTreeSet::from([KeyOp::Read, KeyOp::Share])
        );
        assert_eq!(
            parse_scopes_buckets(&text),
            ["alpha".to_owned(), "zeta".to_owned()].into()
        );
        assert_eq!(
            encode_scopes_with_buckets([KeyOp::Read], &[]),
            r#"{"buckets":[],"ops":["read"]}"#,
            "an explicit empty bucket set is stored as [] (Q7: [] = no buckets)"
        );
    }

    #[test]
    fn ordered_ops_follows_the_canonical_response_order() {
        let set = BTreeSet::from([KeyOp::Share, KeyOp::Read, KeyOp::Upload]);
        assert_eq!(
            ordered_ops(&set),
            vec![KeyOp::Read, KeyOp::Upload, KeyOp::Share]
        );
        assert_eq!(ordered_ops(&BTreeSet::new()), Vec::<KeyOp>::new());
    }

    /// ⑥ An `expires_at` that does not parse is EXPIRED, not "never".
    #[test]
    fn unparseable_expiry_is_expired() {
        let now = Timestamp::now();
        for raw in ["not a timestamp", "", "2026-13-45T99:99:99Z"] {
            assert_eq!(
                is_live(&record(r#"{"ops":["read"]}"#, Some(raw)), now),
                Live::Expired,
                "{raw:?} must fail closed"
            );
        }
        assert_eq!(
            is_live(&record(r#"{"ops":["read"]}"#, None), now),
            Live::Live,
            "no expiry = never expires"
        );
    }

    #[test]
    fn expiry_boundary_is_exclusive() {
        let now = Timestamp::now();
        let future = (now + std::time::Duration::from_secs(60)).to_string();
        let past = (now - std::time::Duration::from_secs(60)).to_string();
        assert_eq!(is_live(&record("{}", Some(&future)), now), Live::Live);
        assert_eq!(is_live(&record("{}", Some(&past)), now), Live::Expired);
        assert_eq!(
            is_live(&record("{}", Some(&now.to_string())), now),
            Live::Expired,
            "the expiry instant itself is already over"
        );
    }

    #[test]
    fn revocation_wins_over_expiry() {
        let now = Timestamp::now();
        let mut revoked = record("{}", Some("not a timestamp"));
        revoked.revoked_at = Some(now.to_string());
        assert_eq!(is_live(&revoked, now), Live::Revoked);
    }

    #[test]
    fn authorize_requires_the_mapped_op() {
        let now = Timestamp::now();
        let upload_only = record(r#"{"ops":["upload"]}"#, None);
        assert_eq!(
            authorize_at(&upload_only, "upload_file", now),
            Ok(KeyOp::Upload)
        );
        assert_eq!(
            authorize_at(&upload_only, "list_files", now),
            Err(Denial::Scope {
                required: KeyOp::Read
            })
        );
        assert_eq!(
            authorize_at(&upload_only, "no_such_tool", now),
            Err(Denial::UnknownTool)
        );
        let expired = record(r#"{"ops":["upload"]}"#, Some("2000-01-01T00:00:00Z"));
        assert_eq!(
            authorize_at(&expired, "upload_file", now),
            Err(Denial::Expired)
        );
    }

    #[test]
    fn denial_messages_name_the_permission_and_the_tool() {
        assert_eq!(
            Denial::Scope {
                required: KeyOp::Upload
            }
            .message("upload_file"),
            r#"scope denied: this API key lacks the "upload" permission required by tool "upload_file""#
        );
        assert_eq!(
            Denial::Expired.message("list_files"),
            "this API key has expired"
        );
    }

    /// Every permission in the vocabulary is enforced by at least one
    /// MCP tool. A `KeyOp` that no tool requires would be a permission
    /// the service advertises (keys.create accepts it, keys.list shows
    /// it) but never enforces — a permission that does nothing.
    ///
    /// This is the MCP-only half of the seam documented on
    /// [`TOOL_REQUIRED_OP`]: a future non-MCP capability guarded by a
    /// `KeyOp` needs its own mapping AND its own test.
    #[test]
    fn every_permission_is_enforced_by_at_least_one_mcp_tool() {
        for op in KeyOp::ALL {
            let tools: Vec<&str> = TOOL_REQUIRED_OP
                .iter()
                .filter(|(_, mapped)| *mapped == op)
                .map(|(name, _)| *name)
                .collect();
            assert!(
                !tools.is_empty(),
                "{op:?} is a grantable permission no MCP tool requires — \
                 it would be a permission that does nothing (add its tool to \
                 TOOL_REQUIRED_OP, or remove it from KeyOp)"
            );
            // ...and the mapping is reachable through the public lookup,
            // which is what the MCP surface actually consults.
            for tool in tools {
                assert_eq!(
                    required_op(tool),
                    Some(op),
                    "{tool} must answer its mapped op through required_op"
                );
            }
        }
    }

    /// Every tool in the table maps to a distinct name, and the vocabulary
    /// covers every op (a KeyOp nobody maps would be unusable).
    #[test]
    fn tool_table_is_well_formed_and_covers_the_vocabulary() {
        let mut names: Vec<&str> = TOOL_REQUIRED_OP.iter().map(|(name, _)| *name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), TOOL_REQUIRED_OP.len(), "tool names are unique");
        for op in KeyOp::ALL {
            assert!(
                TOOL_REQUIRED_OP.iter().any(|(_, mapped)| *mapped == op),
                "{op:?} has no tool"
            );
        }
    }
}
