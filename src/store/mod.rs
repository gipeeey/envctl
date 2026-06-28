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
}
