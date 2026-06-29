pub mod registry;

use crate::crypto::{self, Identity};
use anyhow::{Context, Result};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize, Default)]
struct EnvFile {
    #[serde(default)]
    vars: IndexMap<String, String>,
}

pub struct Store {
    repos_dir: PathBuf,
    identity: Identity,
}

impl Store {
    pub fn new(config_dir: &Path, identity: Identity) -> Self {
        Self {
            repos_dir: config_dir.join("repos"),
            identity,
        }
    }

    fn repo_path(&self, rs: &str, repo: &str) -> PathBuf {
        self.repos_dir.join(rs).join(format!("{repo}.toml.age"))
    }

    pub fn load(&self, rs: &str, repo: &str) -> Result<IndexMap<String, String>> {
        let path = self.repo_path(rs, repo);
        if !path.exists() {
            return Ok(IndexMap::new());
        }
        let ciphertext = std::fs::read(&path)
            .with_context(|| format!("read store: {}", path.display()))?;
        let plaintext = crypto::decrypt(&ciphertext, &self.identity)
            .with_context(|| format!("decrypt store for {rs}/{repo}"))?;
        let env: EnvFile = toml::from_str(
            std::str::from_utf8(&plaintext).context("store not valid utf8")?,
        )
        .context("parse store toml")?;
        Ok(env.vars)
    }

    pub fn save(&self, rs: &str, repo: &str, vars: &IndexMap<String, String>) -> Result<()> {
        let path = self.repo_path(rs, repo);
        std::fs::create_dir_all(path.parent().unwrap())?;
        let env = EnvFile { vars: vars.clone() };
        let plaintext = toml::to_string(&env).context("serialize store")?;
        let ciphertext = crypto::encrypt(plaintext.as_bytes(), &self.identity)
            .with_context(|| format!("encrypt store for {rs}/{repo}"))?;
        std::fs::write(&path, ciphertext)
            .with_context(|| format!("write store for {rs}/{repo}"))?;
        Ok(())
    }

    pub fn delete(&self, rs: &str, repo: &str) -> Result<()> {
        let path = self.repo_path(rs, repo);
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }

    /// Scan store dir and return all (rs, repo) pairs that have store files.
    pub fn list_all(&self) -> Result<Vec<(String, String)>> {
        let mut out = Vec::new();
        if !self.repos_dir.exists() {
            return Ok(out);
        }
        for rs_entry in std::fs::read_dir(&self.repos_dir)? {
            let rs_entry = rs_entry?;
            if !rs_entry.file_type()?.is_dir() {
                continue;
            }
            let rs_name = rs_entry.file_name().to_string_lossy().into_owned();
            for repo_entry in std::fs::read_dir(rs_entry.path())? {
                let repo_entry = repo_entry?;
                let fname = repo_entry.file_name().to_string_lossy().into_owned();
                if let Some(repo_name) = fname.strip_suffix(".toml.age") {
                    out.push((rs_name.clone(), repo_name.to_string()));
                }
            }
        }
        Ok(out)
    }
}
