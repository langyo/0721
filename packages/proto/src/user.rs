//! User records and their validation rules.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A user role. Stored as the lowercase tag; checked in the database.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
#[ts(export, export_to = "../../webui/src/types/")]
pub enum Role {
    /// Full management access.
    Admin,
    /// Regular authenticated access.
    Member,
}

impl Role {
    /// The database tag of this role.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Member => "member",
        }
    }

    /// Parse a role from its database tag.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "admin" => Some(Role::Admin),
            "member" => Some(Role::Member),
            _ => None,
        }
    }
}

/// A stored user, as seen by the data layer.
///
/// `password_hash` is the transition-era credential (argon2id). It is
/// nullable because the OAuth wave (W4) makes password login optional.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserRecord {
    /// Primary key.
    pub id: Uuid,
    /// Login name, unique.
    pub username: String,
    /// Human-facing name.
    pub display_name: String,
    /// Admin or member.
    pub role: Role,
    /// Argon2id hash; `None` = no password login for this user.
    #[serde(skip_serializing)]
    pub password_hash: Option<String>,
    /// Account creation time.
    pub created_at: Timestamp,
}

/// The public projection of a user (never leaks the password hash).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../webui/src/types/")]
pub struct UserDto {
    /// Primary key.
    pub id: Uuid,
    /// Login name.
    pub username: String,
    /// Human-facing name.
    pub display_name: String,
    /// Admin or member.
    pub role: Role,
    /// Account creation time.
    #[ts(type = "string")]
    pub created_at: Timestamp,
}

impl From<UserRecord> for UserDto {
    fn from(record: UserRecord) -> Self {
        Self {
            id: record.id,
            username: record.username,
            display_name: record.display_name,
            role: record.role,
            created_at: record.created_at,
        }
    }
}

/// Parameters for creating a user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewUser {
    /// Login name (validated by [`validate_username`]).
    pub username: String,
    /// Plaintext password, hashed by the caller before storage.
    pub password: String,
    /// Human-facing name; defaults to the username.
    pub display_name: Option<String>,
    /// Role; defaults to member.
    pub role: Option<Role>,
}

/// Why a username is not acceptable.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("username must be 3..=64 chars of [A-Za-z0-9_-], got {value:?}")]
pub struct InvalidUsername {
    /// The rejected value.
    pub value: String,
}

/// Usernames: 3..=64 characters of `[A-Za-z0-9_-]`.
///
/// # Errors
///
/// Returns [`InvalidUsername`] when outside that shape.
pub fn validate_username(value: &str) -> Result<(), InvalidUsername> {
    let len = value.chars().count();
    let shape = (3..=64).contains(&len)
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if shape {
        Ok(())
    } else {
        Err(InvalidUsername {
            value: value.to_owned(),
        })
    }
}

/// Why a display name is not acceptable.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("display name must be 1..=64 chars, got {len} chars")]
pub struct InvalidDisplayName {
    /// The character count.
    pub len: usize,
}

/// Display names: 1..=64 characters (trimmed).
///
/// # Errors
///
/// Returns [`InvalidDisplayName`] when outside that range.
pub fn validate_display_name(value: &str) -> Result<(), InvalidDisplayName> {
    let len = value.trim().chars().count();
    if (1..=64).contains(&len) {
        Ok(())
    } else {
        Err(InvalidDisplayName { len })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_round_trips_tags() {
        for role in [Role::Admin, Role::Member] {
            assert_eq!(Role::parse(role.as_str()), Some(role));
        }
        assert_eq!(Role::parse("root"), None);
    }

    #[test]
    fn role_serializes_lowercase() {
        assert_eq!(serde_json::to_string(&Role::Admin).unwrap(), r#""admin""#);
    }

    #[test]
    fn username_shape() {
        assert!(validate_username("admin").is_ok());
        assert!(validate_username("a_b-C9").is_ok());
        assert!(validate_username("ab").is_err());
        assert!(validate_username("").is_err());
        assert!(validate_username("has space").is_err());
        assert!(validate_username("中文用户").is_err());
        assert!(validate_username(&"x".repeat(65)).is_err());
        assert!(validate_username(&"x".repeat(64)).is_ok());
    }

    #[test]
    fn display_name_shape() {
        assert!(validate_display_name("Administrator").is_ok());
        assert!(validate_display_name("  padded  ").is_ok());
        assert!(validate_display_name("").is_err());
        assert!(validate_display_name("   ").is_err());
        assert!(validate_display_name(&"x".repeat(65)).is_err());
    }

    #[test]
    fn user_dto_never_carries_the_hash() {
        let record = UserRecord {
            id: Uuid::new_v4(),
            username: "admin".to_owned(),
            display_name: "Admin".to_owned(),
            role: Role::Admin,
            password_hash: Some("argon2id$secret".to_owned()),
            created_at: Timestamp::now(),
        };
        let json = serde_json::to_value(UserDto::from(record)).unwrap();
        assert!(json.get("password_hash").is_none());
        assert!(json.get("password").is_none());
    }
}
