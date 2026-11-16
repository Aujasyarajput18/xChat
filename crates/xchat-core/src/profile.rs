use std::{
    fs::{self, File},
    io::{Read, Write},
    path::Path,
};

use serde::{Deserialize, Serialize};

use crate::errors::CoreError;

pub const DEFAULT_INVITE_SCHEME: &str = "xchat://invite";

/// User Profile model representing a human-friendly identity on the decentralized mesh.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct UserProfile {
    pub username: String,
    pub display_name: String,
    pub bio: String,
    pub avatar_color: String,
    pub address: String,
    pub fingerprint: String,
    pub created_at: u64,
    pub last_seen: u64,
}

impl UserProfile {
    pub fn new(
        username: &str,
        display_name: &str,
        address: &str,
        fingerprint: &str,
        now: u64,
    ) -> Result<Self, CoreError> {
        let clean_username = Self::validate_and_normalize_username(username)?;
        Ok(Self {
            username: clean_username,
            display_name: display_name.trim().to_string(),
            bio: "Decentralized mesh operator".to_string(),
            avatar_color: "#d4c4fb".to_string(),
            address: address.to_string(),
            fingerprint: fingerprint.to_string(),
            created_at: now,
            last_seen: now,
        })
    }

    /// Normalizes and validates username (enforces @ prefix, 3-24 chars, valid charset).
    pub fn validate_and_normalize_username(raw: &str) -> Result<String, CoreError> {
        let trimmed = raw.trim();
        let stripped = trimmed.strip_prefix('@').unwrap_or(trimmed);

        if stripped.len() < 3 || stripped.len() > 24 {
            return Err(CoreError::Serialization(
                "Username must be between 3 and 24 characters".into(),
            ));
        }

        if !stripped
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
        {
            return Err(CoreError::Serialization(
                "Username contains invalid characters (allowed: a-z, 0-9, _, .)".into(),
            ));
        }

        Ok(format!("@{}", stripped.to_ascii_lowercase()))
    }

    /// Generate an authentic shareable invite URL.
    pub fn generate_invite_url(&self, current_socket: &str) -> String {
        format!(
            "{}/{}?addr={}&fp={}&sock={}",
            DEFAULT_INVITE_SCHEME,
            self.username.trim_start_matches('@'),
            self.address,
            self.fingerprint,
            current_socket
        )
    }

    /// Parse an incoming invite URL.
    pub fn parse_invite_url(url: &str) -> Result<(String, String, String, String), CoreError> {
        if !url.starts_with(DEFAULT_INVITE_SCHEME) {
            return Err(CoreError::Serialization("Invalid invite URI scheme".into()));
        }

        let after_scheme = &url[DEFAULT_INVITE_SCHEME.len()..];
        let trimmed = after_scheme.trim_start_matches('/');
        let parts: Vec<&str> = trimmed.split('?').collect();
        let username_part = parts
            .first()
            .ok_or_else(|| CoreError::Serialization("Missing username in invite URI".into()))?;

        let username = format!("@{}", username_part.trim_start_matches('@'));
        let mut address = String::new();
        let mut fingerprint = String::new();
        let mut socket = String::new();

        if let Some(query) = parts.get(1) {
            for pair in query.split('&') {
                let kv: Vec<&str> = pair.split('=').collect();
                if kv.len() == 2 {
                    match kv[0] {
                        "addr" => address = kv[1].to_string(),
                        "fp" => fingerprint = kv[1].to_string(),
                        "sock" => socket = kv[1].to_string(),
                        _ => {}
                    }
                }
            }
        }

        if address.is_empty() {
            return Err(CoreError::Serialization(
                "Invite URI missing required address parameter".into(),
            ));
        }

        Ok((username, address, fingerprint, socket))
    }

    /// Persist user profile to local disk.
    pub fn save_to_disk(&self, path: &Path) -> Result<(), CoreError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| CoreError::StorageError(e.to_string()))?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| CoreError::Serialization(e.to_string()))?;
        let mut file = File::create(path).map_err(|e| CoreError::StorageError(e.to_string()))?;
        file.write_all(json.as_bytes())
            .map_err(|e| CoreError::StorageError(e.to_string()))?;
        Ok(())
    }

    /// Load user profile from local disk.
    pub fn load_from_disk(path: &Path) -> Result<Self, CoreError> {
        let mut file = File::open(path).map_err(|e| CoreError::StorageError(e.to_string()))?;
        let mut contents = String::new();
        file.read_to_string(&mut contents)
            .map_err(|e| CoreError::StorageError(e.to_string()))?;
        serde_json::from_str(&contents).map_err(|e| CoreError::Serialization(e.to_string()))
    }
}

/// Represents a peer discovered over zero-configuration local wireless beacon or mesh announcement.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct DiscoveredPeer {
    pub username: String,
    pub display_name: String,
    pub address: String,
    pub fingerprint: String,
    pub socket_addr: String,
    pub discovered_at: u64,
    pub transport_type: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_username_normalization_and_validation() {
        assert_eq!(
            UserProfile::validate_and_normalize_username("alice").unwrap(),
            "@alice"
        );
        assert_eq!(
            UserProfile::validate_and_normalize_username("@Bob_99").unwrap(),
            "@bob_99"
        );
        assert!(UserProfile::validate_and_normalize_username("a").is_err());
        assert!(UserProfile::validate_and_normalize_username("bad space").is_err());
    }

    #[test]
    fn test_invite_url_generation_and_parsing() {
        let profile = UserProfile::new("charlie", "Charlie", "xchat_12345", "fp_98765", 1000).unwrap();
        let invite = profile.generate_invite_url("192.168.1.100:8080");
        assert!(invite.starts_with("xchat://invite/charlie?"));

        let (u, a, fp, sock) = UserProfile::parse_invite_url(&invite).unwrap();
        assert_eq!(u, "@charlie");
        assert_eq!(a, "xchat_12345");
        assert_eq!(fp, "fp_98765");
        assert_eq!(sock, "192.168.1.100:8080");
    }

    #[test]
    fn test_user_profile_disk_persistence() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("profile.json");

        let profile = UserProfile::new("operator", "Mesh Operator", "xchat_abc123", "fp_xyz789", 12345).unwrap();
        profile.save_to_disk(&path).unwrap();

        let loaded = UserProfile::load_from_disk(&path).unwrap();
        assert_eq!(profile, loaded);
    }
}
