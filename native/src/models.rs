use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct Error {
    pub message: String,
    pub code: String,
    pub uncertain: bool,
}

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: "local".into(),
            uncertain: false,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
    }
}
impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::new(error.to_string())
    }
}
impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        Self::new(format!("SQLite : {error}"))
    }
}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub socket_path: String,
    pub pane_id: String,
    pub terminal_id: String,
    pub workspace_id: String,
    pub agent: String,
    pub cwd: String,
    pub session: Option<String>,
}

impl Target {
    pub fn key(&self) -> String {
        let identity = python_string_array(&[
            Some(&self.socket_path),
            Some(&self.terminal_id),
            Some(&self.agent),
            self.session.as_deref(),
        ]);
        format!("{:x}", Sha256::digest(identity.as_bytes()))
    }
    pub fn lock_key(&self) -> String {
        let identity = python_string_array(&[Some(&self.socket_path), Some(&self.terminal_id)]);
        format!("{:x}", Sha256::digest(identity.as_bytes()))
    }
    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string(self)?)
    }
    pub fn from_json(raw: &str) -> Result<Self> {
        let target: Self = serde_json::from_str(raw)?;
        if [
            &target.socket_path,
            &target.pane_id,
            &target.terminal_id,
            &target.workspace_id,
            &target.agent,
        ]
        .iter()
        .any(|value| value.is_empty())
        {
            return Err(Error::new("Destination invalide."));
        }
        Ok(target)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    pub target: Target,
    pub status: String,
    pub name: String,
    pub workspace_label: String,
    pub tab_label: String,
    pub session_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrSession {
    pub name: String,
    pub socket_path: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Composer {
    pub body: String,
    pub editing_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub id: String,
    pub target_key: String,
    pub body: String,
    pub state: String,
    pub position: i64,
    pub error: Option<String>,
}

/// Preserve Python's default JSON separators and ensure_ascii for existing database keys.
pub fn python_string_array(values: &[Option<&str>]) -> String {
    let encoded = values
        .iter()
        .map(|value| match value {
            None => "null".to_string(),
            Some(value) => {
                let quoted =
                    serde_json::to_string(value).expect("a JSON string is always serializable");
                let mut ascii = String::with_capacity(quoted.len());
                for character in quoted.chars() {
                    if character <= '~' {
                        ascii.push(character);
                    } else {
                        let mut units = [0; 2];
                        for unit in character.encode_utf16(&mut units) {
                            ascii.push_str(&format!("\\u{unit:04x}"));
                        }
                    }
                }
                ascii
            }
        })
        .collect::<Vec<_>>();
    format!("[{}]", encoded.join(", "))
}

pub fn unique_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{nanos:x}{:08x}{:08x}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_identity_keeps_spacing_and_unicode_escapes() {
        assert_eq!(
            python_string_array(&[Some("café 🎯"), None]),
            r#"["caf\u00e9 \ud83c\udfaf", null]"#
        );
    }

    #[test]
    fn existing_python_target_key_is_preserved() {
        let target = Target {
            socket_path: "/tmp/café.sock".into(),
            pane_id: "p1".into(),
            terminal_id: "term-é".into(),
            workspace_id: "w1".into(),
            agent: "codex".into(),
            cwd: "/project".into(),
            session: Some(r#"["test", "id", "s1"]"#.into()),
        };
        assert_eq!(
            target.key(),
            "994f19c19c9e34e90cf077229ecf1799da78517e451e9c3739b6765be7265f1d"
        );
    }

    #[test]
    fn malformed_target_never_accepts_empty_terminal() {
        let raw = r#"{"socket_path":"/tmp/s","pane_id":"p","terminal_id":"","workspace_id":"w","agent":"codex","cwd":"","session":null}"#;
        assert!(Target::from_json(raw).is_err());
    }
}
