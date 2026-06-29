use anyhow::{Context, Result, bail};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct RsEntry {
    pub ip: String,
    #[serde(default)]
    pub repos: IndexMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct Registry {
    #[serde(default)]
    pub repos: IndexMap<String, String>,
    #[serde(default)]
    pub rs: IndexMap<String, RsEntry>,
}

impl Registry {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let s = std::fs::read_to_string(path)
            .with_context(|| format!("read registry: {}", path.display()))?;
        toml::from_str(&s).context("parse registry")
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let s = toml::to_string(self).context("serialize registry")?;
        std::fs::write(path, s)
            .with_context(|| format!("write registry: {}", path.display()))?;
        Ok(())
    }

    pub fn add_rs(&mut self, name: String, ip: String) {
        self.rs.entry(name).or_insert_with(|| RsEntry { ip, repos: IndexMap::new() });
    }

    pub fn register_repo(&mut self, rs: &str, repo_name: String, path: PathBuf) -> bool {
        if let Some(entry) = self.rs.get_mut(rs) {
            entry.repos.insert(repo_name, path.to_string_lossy().to_string());
            true
        } else {
            false
        }
    }

    pub fn get_repo_path(&self, rs: &str, repo: &str) -> Option<PathBuf> {
        // global repos take priority over RS-specific
        if let Some(p) = self.repos.get(repo) {
            return Some(PathBuf::from(p));
        }
        self.rs.get(rs)?.repos.get(repo).map(PathBuf::from)
    }

    pub fn add_repo(&mut self, name: String, path: PathBuf) {
        self.repos.insert(name, path.to_string_lossy().to_string());
    }

    pub fn remove_repo(&mut self, name: &str) -> bool {
        self.repos.shift_remove(name).is_some()
    }

    pub fn update_repo(&mut self, name: &str, path: PathBuf) -> Result<()> {
        if !self.repos.contains_key(name) {
            bail!("repo not found: {name}");
        }
        self.repos.insert(name.to_string(), path.to_string_lossy().to_string());
        Ok(())
    }

    pub fn find_by_ip(&self, ip: &str) -> Option<&str> {
        self.rs.iter().find(|(_, e)| e.ip == ip).map(|(n, _)| n.as_str())
    }
}
