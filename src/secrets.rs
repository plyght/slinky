use age::{Decryptor, Encryptor};
use regex::Regex;
use secrecy::Secret as SecrecySecret;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum SecretError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Regex error: {0}")]
    Regex(#[from] regex::Error),

    #[error("Encryption error: {0}")]
    Encryption(String),

    #[error("Decryption error: {0}")]
    #[allow(dead_code)]
    Decryption(String),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Secret not found: {0}")]
    #[allow(dead_code)]
    SecretNotFound(String),

    #[error("Template file not found: {0}")]
    #[allow(dead_code)]
    TemplateNotFound(String),

    #[error("Invalid passphrase")]
    #[allow(dead_code)]
    InvalidPassphrase,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Secret {
    pub name: String,
    #[serde(skip)]
    pub value: String,
    pub file: PathBuf,
    pub line_number: usize,
}

impl Secret {
    pub fn new(name: String, value: String, file: PathBuf, line_number: usize) -> Self {
        Self {
            name,
            value,
            file,
            line_number,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct EncryptedData {
    secrets: HashMap<String, String>,
    metadata: HashMap<String, SecretMetadata>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SecretMetadata {
    file: PathBuf,
    line_number: usize,
}

pub struct SecretStore {
    encrypted_data: Vec<u8>,
    secrets_path: PathBuf,
}

impl SecretStore {
    pub fn new(secrets_path: PathBuf) -> Self {
        Self {
            encrypted_data: Vec::new(),
            secrets_path,
        }
    }

    #[allow(dead_code)]
    pub fn load(secrets_path: &Path) -> Result<Self, SecretError> {
        let encrypted_data = fs::read(secrets_path)?;
        Ok(Self {
            encrypted_data,
            secrets_path: secrets_path.to_path_buf(),
        })
    }

    pub fn save(&self) -> Result<(), SecretError> {
        if let Some(parent) = self.secrets_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&self.secrets_path, &self.encrypted_data)?;
        Ok(())
    }

    #[allow(dead_code)]
    fn decrypt_with_passphrase(
        &self,
        passphrase: &str,
    ) -> Result<HashMap<String, String>, SecretError> {
        let decryptor = match Decryptor::new(&self.encrypted_data[..]) {
            Ok(Decryptor::Passphrase(d)) => d,
            Ok(_) => {
                return Err(SecretError::Decryption(
                    "Unexpected decryptor type".to_string(),
                ))
            }
            Err(e) => return Err(SecretError::Decryption(format!("Decryption failed: {}", e))),
        };

        let mut decrypted = Vec::new();
        let mut reader = decryptor
            .decrypt(&SecrecySecret::new(passphrase.to_string()), None)
            .map_err(|e| SecretError::Decryption(format!("Failed to decrypt: {}", e)))?;

        std::io::copy(&mut reader, &mut decrypted).map_err(|e| {
            SecretError::Decryption(format!("Failed to read decrypted data: {}", e))
        })?;

        let encrypted_data: EncryptedData = serde_json::from_slice(&decrypted)?;
        Ok(encrypted_data.secrets)
    }
}

pub fn scan_file_for_secrets(path: &Path) -> Result<Vec<Secret>, SecretError> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);

    let sensitive_patterns = vec![
        "API_KEY",
        "APIKEY",
        "TOKEN",
        "PASSWORD",
        "PASSWD",
        "PWD",
        "SECRET",
        "AUTH",
        "CREDENTIAL",
        "PRIVATE_KEY",
        "ACCESS_KEY",
        "SESSION",
    ];

    let pattern_str = sensitive_patterns
        .iter()
        .map(|p| format!(r"(?i){}S?", p))
        .collect::<Vec<_>>()
        .join("|");

    let bash_regex = Regex::new(
        r#"^\s*(?:export\s+)?([A-Z_][A-Z0-9_]*)\s*=\s*["']?([^"'\n]+?)["']?\s*(?:#.*)?$"#,
    )?;
    let sensitive_regex = Regex::new(&format!(r"(?i)(?:{})", pattern_str))?;

    let fish_regex = Regex::new(
        r#"^\s*set\s+(?:-[gx]+\s+)?([A-Z_][A-Z0-9_]*)\s+["']?([^"'\n]+?)["']?\s*(?:#.*)?$"#,
    )?;

    let mut secrets = Vec::new();

    for (line_num, line) in reader.lines().enumerate() {
        let line = line?;
        let line_number = line_num + 1;

        if let Some(caps) = bash_regex.captures(&line) {
            if let (Some(name), Some(value)) = (caps.get(1), caps.get(2)) {
                let name_str = name.as_str();
                let value_str = value.as_str();
                if sensitive_regex.is_match(name_str)
                    && !value_str.is_empty()
                    && !value_str.starts_with('$')
                {
                    secrets.push(Secret::new(
                        name_str.to_string(),
                        value_str.to_string(),
                        path.to_path_buf(),
                        line_number,
                    ));
                }
            }
        } else if let Some(caps) = fish_regex.captures(&line) {
            if let (Some(name), Some(value)) = (caps.get(1), caps.get(2)) {
                let name_str = name.as_str();
                let value_str = value.as_str();
                if sensitive_regex.is_match(name_str)
                    && !value_str.is_empty()
                    && !value_str.starts_with('$')
                {
                    secrets.push(Secret::new(
                        name_str.to_string(),
                        value_str.to_string(),
                        path.to_path_buf(),
                        line_number,
                    ));
                }
            }
        }
    }

    Ok(secrets)
}

pub fn create_template(file: &Path, secrets: &[Secret]) -> Result<PathBuf, SecretError> {
    let file_content = fs::read_to_string(file)?;
    let lines: Vec<&str> = file_content.lines().collect();
    let mut templated_lines = lines.clone();

    for secret in secrets {
        if secret.line_number > 0 && secret.line_number <= templated_lines.len() {
            let line_idx = secret.line_number - 1;
            let original_line = templated_lines[line_idx];

            let placeholder = format!("${{{}}}", secret.name);

            let templated_line = original_line.replace(&secret.value, &placeholder);

            templated_lines[line_idx] = Box::leak(templated_line.into_boxed_str());
        }
    }

    let template_path = file.with_extension(
        file.extension()
            .and_then(|e| e.to_str())
            .map(|e| format!("{}.template", e))
            .unwrap_or_else(|| "template".to_string()),
    );

    let mut output_file = File::create(&template_path)?;
    for (i, line) in templated_lines.iter().enumerate() {
        if i > 0 {
            writeln!(output_file)?;
        }
        write!(output_file, "{}", line)?;
    }

    Ok(template_path)
}

pub fn encrypt_secrets(secrets: &[Secret], passphrase: &str) -> Result<SecretStore, SecretError> {
    let mut secret_map = HashMap::new();
    let mut metadata = HashMap::new();

    for secret in secrets {
        secret_map.insert(secret.name.clone(), secret.value.clone());
        metadata.insert(
            secret.name.clone(),
            SecretMetadata {
                file: secret.file.clone(),
                line_number: secret.line_number,
            },
        );
    }

    let encrypted_data = EncryptedData {
        secrets: secret_map,
        metadata,
    };

    let json_data = serde_json::to_vec(&encrypted_data)?;

    let encryptor = Encryptor::with_user_passphrase(SecrecySecret::new(passphrase.to_string()));

    let mut encrypted = Vec::new();
    let mut writer = encryptor
        .wrap_output(&mut encrypted)
        .map_err(|e| SecretError::Encryption(format!("Failed to create encryptor: {}", e)))?;

    writer
        .write_all(&json_data)
        .map_err(|e| SecretError::Encryption(format!("Failed to write encrypted data: {}", e)))?;

    writer
        .finish()
        .map_err(|e| SecretError::Encryption(format!("Failed to finish encryption: {}", e)))?;

    let secrets_dir = directories::BaseDirs::new()
        .ok_or_else(|| {
            SecretError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Could not determine home directory",
            ))
        })?
        .data_local_dir()
        .join("slinky");

    fs::create_dir_all(&secrets_dir)?;
    let secrets_path = secrets_dir.join("secrets.age");

    let mut store = SecretStore::new(secrets_path);
    store.encrypted_data = encrypted;
    store.save()?;

    Ok(store)
}

#[allow(dead_code)]
pub fn decrypt_and_substitute(
    template: &Path,
    store: &SecretStore,
    passphrase: &str,
) -> Result<(), SecretError> {
    if !template.exists() {
        return Err(SecretError::TemplateNotFound(
            template.display().to_string(),
        ));
    }

    let secrets = store.decrypt_with_passphrase(passphrase)?;

    let template_content = fs::read_to_string(template)?;

    let mut output_content = template_content.clone();
    for (name, value) in &secrets {
        let placeholder = format!("${{{}}}", name);
        output_content = output_content.replace(&placeholder, value);
    }

    let output_path = if template.to_string_lossy().ends_with(".template") {
        PathBuf::from(template.to_string_lossy().trim_end_matches(".template"))
    } else {
        template.with_extension("")
    };

    fs::write(&output_path, output_content)?;

    Ok(())
}

#[allow(dead_code)]
pub fn get_default_secrets_path() -> Result<PathBuf, SecretError> {
    let base_dirs = directories::BaseDirs::new().ok_or_else(|| {
        SecretError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Could not determine home directory",
        ))
    })?;

    Ok(base_dirs
        .data_local_dir()
        .join("slinky")
        .join("secrets.age"))
}

pub fn scan_shell_configs() -> Result<Vec<PathBuf>, SecretError> {
    let home = directories::BaseDirs::new()
        .ok_or_else(|| {
            SecretError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Could not determine home directory",
            ))
        })?
        .home_dir()
        .to_path_buf();

    let config_files = vec![
        home.join(".zshrc"),
        home.join(".bashrc"),
        home.join(".bash_profile"),
        home.join(".profile"),
        home.join(".config/fish/config.fish"),
    ];

    let existing_files: Vec<PathBuf> = config_files.into_iter().filter(|p| p.exists()).collect();

    Ok(existing_files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_scan_bash_export() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "export API_KEY=secret123").unwrap();
        writeln!(file, "export NORMAL_VAR=value").unwrap();
        writeln!(file, "export GITHUB_TOKEN=ghp_abc123").unwrap();
        file.flush().unwrap();

        let secrets = scan_file_for_secrets(file.path()).unwrap();
        assert_eq!(secrets.len(), 2);
        assert!(secrets.iter().any(|s| s.name == "API_KEY"));
        assert!(secrets.iter().any(|s| s.name == "GITHUB_TOKEN"));
    }

    #[test]
    fn test_scan_fish_syntax() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "set -gx DATABASE_PASSWORD hunter2").unwrap();
        writeln!(file, "set -x AUTH_TOKEN abc123").unwrap();
        file.flush().unwrap();

        let secrets = scan_file_for_secrets(file.path()).unwrap();
        assert_eq!(secrets.len(), 2);
        assert!(secrets.iter().any(|s| s.name == "DATABASE_PASSWORD"));
        assert!(secrets.iter().any(|s| s.name == "AUTH_TOKEN"));
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let secrets = vec![Secret::new(
            "TEST_SECRET".to_string(),
            "sensitive_value".to_string(),
            PathBuf::from("/test/.zshrc"),
            1,
        )];

        let passphrase = "test_passphrase_123";
        let store = encrypt_secrets(&secrets, passphrase).unwrap();

        let decrypted = store.decrypt_with_passphrase(passphrase).unwrap();
        assert_eq!(decrypted.get("TEST_SECRET").unwrap(), "sensitive_value");
    }

    #[test]
    fn test_create_template() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "export API_KEY=secret123").unwrap();
        writeln!(file, "export NORMAL=value").unwrap();
        file.flush().unwrap();

        let secrets = vec![Secret::new(
            "API_KEY".to_string(),
            "secret123".to_string(),
            file.path().to_path_buf(),
            1,
        )];

        let template_path = create_template(file.path(), &secrets).unwrap();
        let content = fs::read_to_string(&template_path).unwrap();

        assert!(content.contains("${API_KEY}"));
        assert!(!content.contains("secret123"));
        assert!(content.contains("NORMAL=value"));
    }
}
