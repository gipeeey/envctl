mod cli;
mod crypto;
mod env_file;
mod store;
mod tui;

use anyhow::{Context, Result, bail};
use indexmap::IndexMap;
use toml;
use clap::Parser;
use cli::{Cli, Command, ExportTarget, ListTarget, RepoCommand};
use crypto::Identity;
use similar::{ChangeTag, TextDiff};
use std::path::{Path, PathBuf};
use store::{Store, registry::Registry};
use walkdir::WalkDir;

fn config_dir() -> Result<PathBuf> {
    dirs::config_dir()
        .map(|d| d.join("envctl"))
        .context("cannot determine config dir")
}

fn identity_path(config: &Path) -> PathBuf {
    config.join("identity.age")
}

fn registry_path(config: &Path) -> PathBuf {
    config.join("registry.toml")
}

fn load_identity(config: &Path) -> Result<Identity> {
    let path = identity_path(config);
    if !path.exists() {
        bail!("not initialized — run `envctl init` first");
    }
    Identity::from_file(&path)
}

fn load_store(config: &Path) -> Result<Store> {
    let identity = load_identity(config)?;
    Ok(Store::new(config, identity))
}

fn load_registry(config: &Path) -> Result<Registry> {
    Registry::load(&registry_path(config))
}

fn save_registry(config: &Path, reg: &Registry) -> Result<()> {
    reg.save(&registry_path(config))
}

fn mask(value: &str) -> String {
    if value.len() <= 4 {
        "••••".to_string()
    } else {
        format!("{}••••", &value[..2])
    }
}

fn cmd_init(config: &Path) -> Result<()> {
    std::fs::create_dir_all(config)?;
    std::fs::create_dir_all(config.join("repos"))?;

    let id_path = identity_path(config);
    if id_path.exists() {
        println!("already initialized: {}", config.display());
        return Ok(());
    }

    let identity = Identity::generate();
    std::fs::write(&id_path, identity.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&id_path, std::fs::Permissions::from_mode(0o600))?;
    }

    println!("initialized: {}", config.display());
    println!("IMPORTANT: back up your key → {}", id_path.display());
    Ok(())
}

fn cmd_rs_add(config: &Path, name: &str, ip: &str) -> Result<()> {
    let mut reg = load_registry(config)?;
    if reg.rs.contains_key(name) {
        println!("already exists: {name}");
        return Ok(());
    }
    reg.add_rs(name.to_string(), ip.to_string());
    save_registry(config, &reg)?;
    println!("added RS: {name} ({ip})");
    Ok(())
}

fn cmd_rs_list(config: &Path) -> Result<()> {
    let reg = load_registry(config)?;
    if reg.rs.is_empty() {
        println!("no RS registered — use `envctl rs add <name> --ip <ip>`");
        return Ok(());
    }
    println!("{:<30} {:<20} REPOS", "NAME", "IP");
    println!("{}", "─".repeat(60));
    for (name, entry) in &reg.rs {
        println!("{:<30} {:<20} {}", name, entry.ip, entry.repos.len());
    }
    Ok(())
}

fn cmd_rs_search(config: &Path, query: &str) -> Result<()> {
    let reg = load_registry(config)?;
    let q = query.to_lowercase();
    let matches: Vec<_> = reg.rs.iter()
        .filter(|(name, entry)| name.to_lowercase().contains(&q) || entry.ip.to_lowercase().contains(&q))
        .collect();
    if matches.is_empty() {
        println!("no RS matching: {query}");
        return Ok(());
    }
    println!("{:<30} {:<20} REPOS", "NAME", "IP");
    println!("{}", "─".repeat(60));
    for (name, entry) in matches {
        println!("{:<30} {:<20} {}", name, entry.ip, entry.repos.len());
    }
    Ok(())
}

fn cmd_rs_remove(config: &Path, name: &str) -> Result<()> {
    let mut reg = load_registry(config)?;
    if reg.rs.shift_remove(name).is_none() {
        bail!("RS not found: {name}");
    }
    save_registry(config, &reg)?;
    println!("removed RS: {name}");
    Ok(())
}

fn cmd_register(config: &Path, path: &str, rs: &str, name: Option<&str>) -> Result<()> {
    let abs = std::fs::canonicalize(path)
        .with_context(|| format!("path not found: {path}"))?;

    let repo_name = name
        .map(str::to_string)
        .or_else(|| abs.file_name().map(|n| n.to_string_lossy().to_string()))
        .context("cannot determine repo name")?;

    let mut reg = load_registry(config)?;
    if !reg.rs.contains_key(rs) {
        bail!("RS not found: {rs} — use `envctl rs add {rs} --ip <ip>` first");
    }
    if reg.rs[rs].repos.contains_key(&repo_name) {
        println!("already registered: {rs}/{repo_name}");
        return Ok(());
    }
    reg.register_repo(rs, repo_name.clone(), abs.clone());
    save_registry(config, &reg)?;
    println!("registered: {rs}/{repo_name} → {}", abs.display());
    Ok(())
}

fn cmd_scan(config: &Path, dir: &str, rs: &str) -> Result<()> {
    let base = std::fs::canonicalize(dir)
        .with_context(|| format!("dir not found: {dir}"))?;

    let mut reg = load_registry(config)?;
    if !reg.rs.contains_key(rs) {
        bail!("RS not found: {rs} — use `envctl rs add {rs} --ip <ip>` first");
    }

    let mut found: Vec<(String, PathBuf)> = Vec::new();
    for entry in WalkDir::new(&base).min_depth(1).max_depth(3) {
        let entry = entry?;
        if entry.file_name() == ".git" && entry.file_type().is_dir() {
            let repo_path = entry.path().parent().unwrap().to_path_buf();
            let name = repo_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            found.push((name, repo_path));
        }
    }

    if found.is_empty() {
        println!("no git repos found in {}", base.display());
        return Ok(());
    }

    println!("found {} repos:", found.len());
    for (name, path) in &found {
        println!("  {name} → {}", path.display());
    }

    print!("\nregister all under {rs}? [y/N] ");
    use std::io::BufRead;
    std::io::Write::flush(&mut std::io::stdout())?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    if !line.trim().eq_ignore_ascii_case("y") {
        println!("aborted");
        return Ok(());
    }

    let mut added = 0;
    for (name, path) in found {
        if !reg.rs[rs].repos.contains_key(&name) {
            reg.register_repo(rs, name.clone(), path);
            println!("  + {name}");
            added += 1;
        } else {
            println!("  ~ {name} (already registered)");
        }
    }
    save_registry(config, &reg)?;
    println!("registered {added} repos under {rs}");
    Ok(())
}

fn cmd_list_rs(config: &Path) -> Result<()> {
    cmd_rs_list(config)
}

fn cmd_list_repos(config: &Path, rs_filter: Option<&str>) -> Result<()> {
    let reg = load_registry(config)?;
    if reg.rs.is_empty() {
        println!("no RS registered");
        return Ok(());
    }
    println!("{:<20} {:<30} {}", "RS", "REPO", "PATH");
    println!("{}", "─".repeat(80));
    for (rs_name, entry) in &reg.rs {
        if let Some(f) = rs_filter {
            if rs_name != f {
                continue;
            }
        }
        for (repo_name, path) in &entry.repos {
            println!("{:<20} {:<30} {}", rs_name, repo_name, path);
        }
    }
    Ok(())
}

fn cmd_list_vars(config: &Path, rs: &str, repo: &str, reveal: bool) -> Result<()> {
    let store = load_store(config)?;
    let vars = store.load(rs, repo)?;
    if vars.is_empty() {
        println!("no vars for {rs}/{repo}");
        return Ok(());
    }
    println!("{:<40} {}", "KEY", "VALUE");
    println!("{}", "─".repeat(70));
    for (k, v) in &vars {
        let display = if reveal { v.clone() } else { mask(v) };
        println!("{:<40} {}", k, display);
    }
    Ok(())
}

fn cmd_set(config: &Path, rs: &str, repo: &str, pair: &str) -> Result<()> {
    let (key, value) = pair
        .split_once('=')
        .with_context(|| format!("expected KEY=VALUE, got: {pair}"))?;
    let key = key.trim().to_string();
    let value = value.trim_matches('"').trim_matches('\'').to_string();

    let store = load_store(config)?;
    let mut vars = store.load(rs, repo)?;
    vars.insert(key.clone(), value);
    store.save(rs, repo, &vars)?;
    println!("set {key} in {rs}/{repo}");
    Ok(())
}

fn cmd_get(config: &Path, rs: &str, repo: &str, key: &str, reveal: bool) -> Result<()> {
    let store = load_store(config)?;
    let vars = store.load(rs, repo)?;
    match vars.get(key) {
        Some(v) => println!("{}", if reveal { v.clone() } else { mask(v) }),
        None => bail!("key not found: {key} in {rs}/{repo}"),
    }
    Ok(())
}

fn cmd_delete(config: &Path, rs: &str, repo: &str, key: &str) -> Result<()> {
    let store = load_store(config)?;
    let mut vars = store.load(rs, repo)?;
    if vars.shift_remove(key).is_none() {
        bail!("key not found: {key} in {rs}/{repo}");
    }
    store.save(rs, repo, &vars)?;
    println!("deleted {key} from {rs}/{repo}");
    Ok(())
}

fn cmd_import(config: &Path, rs: &str, repo: &str, env_file: &str) -> Result<()> {
    let content = std::fs::read_to_string(env_file)
        .with_context(|| format!("read {env_file}"))?;
    let new_vars = env_file::parse(&content);
    let count = new_vars.len();

    let store = load_store(config)?;
    let mut vars = store.load(rs, repo)?;
    vars.extend(new_vars);
    store.save(rs, repo, &vars)?;
    println!("imported {count} vars into {rs}/{repo}");
    Ok(())
}

fn cmd_edit(config: &Path, rs: &str, repo: &str) -> Result<()> {
    let store = load_store(config)?;
    let vars = store.load(rs, repo)?;
    let content = env_file::serialize(&vars);

    let mut tmp = tempfile::Builder::new()
        .prefix(&format!("envctl-{rs}-{repo}-"))
        .suffix(".env")
        .tempfile()
        .context("create temp file")?;
    use std::io::Write;
    tmp.write_all(content.as_bytes())?;
    let tmp_path = tmp.path().to_path_buf();

    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| if cfg!(windows) { "notepad".to_string() } else { "nano".to_string() });
    let status = std::process::Command::new(&editor)
        .arg(&tmp_path)
        .status()
        .with_context(|| format!("launch editor: {editor}"))?;

    if !status.success() {
        bail!("editor exited with error");
    }

    let updated = std::fs::read_to_string(&tmp_path).context("read temp file")?;
    let new_vars = env_file::parse(&updated);
    let count = new_vars.len();
    store.save(rs, repo, &new_vars)?;
    println!("saved {count} vars for {rs}/{repo}");
    Ok(())
}

fn cmd_apply(config: &Path, rs: &str, repo: &str) -> Result<()> {
    let reg = load_registry(config)?;
    let repo_path = reg
        .get_repo_path(rs, repo)
        .with_context(|| format!("repo not registered: {rs}/{repo}"))?;

    let store = load_store(config)?;
    let vars = store.load(rs, repo)?;
    let content = env_file::serialize(&vars);
    let env_path = repo_path.join(".env");
    std::fs::write(&env_path, &content)?;
    println!("applied {} vars → {}", vars.len(), env_path.display());
    Ok(())
}

fn cmd_apply_all(config: &Path) -> Result<()> {
    let reg = load_registry(config)?;
    if reg.rs.is_empty() {
        println!("no RS registered");
        return Ok(());
    }
    let store = load_store(config)?;
    let mut ok = 0;
    let mut errs = 0;
    for (rs_name, entry) in &reg.rs {
        for (repo_name, path_str) in &entry.repos {
            let vars = match store.load(rs_name, repo_name) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("  ✗ {rs_name}/{repo_name}: {e}");
                    errs += 1;
                    continue;
                }
            };
            if vars.is_empty() {
                println!("  ~ {rs_name}/{repo_name}: no vars, skip");
                continue;
            }
            let content = env_file::serialize(&vars);
            let env_path = PathBuf::from(path_str).join(".env");
            match std::fs::write(&env_path, content) {
                Ok(_) => {
                    println!("  ✓ {rs_name}/{repo_name} ({} vars)", vars.len());
                    ok += 1;
                }
                Err(e) => {
                    eprintln!("  ✗ {rs_name}/{repo_name}: {e}");
                    errs += 1;
                }
            }
        }
    }
    println!("\napplied: {ok}  errors: {errs}");
    Ok(())
}

fn cmd_repo_add(config: &Path, name: &str, path: &str) -> Result<()> {
    let abs = std::fs::canonicalize(path)
        .with_context(|| format!("path not found: {path}"))?;
    let mut reg = load_registry(config)?;
    if reg.repos.contains_key(name) {
        println!("already exists: {name} → use `envctl repo update` to change path");
        return Ok(());
    }
    reg.add_repo(name.to_string(), abs.clone());
    save_registry(config, &reg)?;
    println!("added repo: {name} → {}", abs.display());
    Ok(())
}

fn cmd_repo_list(config: &Path) -> Result<()> {
    let reg = load_registry(config)?;
    if reg.repos.is_empty() {
        println!("no global repos — use `envctl repo add <name> --path <path>`");
        return Ok(());
    }
    println!("{:<30} {}", "NAME", "PATH");
    println!("{}", "─".repeat(70));
    for (name, path) in &reg.repos {
        println!("{:<30} {}", name, path);
    }
    Ok(())
}

fn cmd_repo_remove(config: &Path, name: &str) -> Result<()> {
    let mut reg = load_registry(config)?;
    if !reg.remove_repo(name) {
        bail!("repo not found: {name}");
    }
    save_registry(config, &reg)?;
    println!("removed repo: {name}");
    Ok(())
}

fn cmd_repo_update(config: &Path, name: &str, path: &str) -> Result<()> {
    let abs = std::fs::canonicalize(path)
        .with_context(|| format!("path not found: {path}"))?;
    let mut reg = load_registry(config)?;
    reg.update_repo(name, abs.clone())?;
    save_registry(config, &reg)?;
    println!("updated repo: {name} → {}", abs.display());
    Ok(())
}

fn cmd_diff(config: &Path, rs: &str, repo: &str) -> Result<()> {
    let reg = load_registry(config)?;
    let repo_path = reg
        .get_repo_path(rs, repo)
        .with_context(|| format!("repo not registered: {rs}/{repo}"))?;

    let store = load_store(config)?;
    let vars = store.load(rs, repo)?;
    let store_content = env_file::serialize(&vars);

    let env_path = repo_path.join(".env");
    let disk_content = if env_path.exists() {
        std::fs::read_to_string(&env_path)?
    } else {
        String::new()
    };

    if store_content == disk_content {
        println!("{rs}/{repo}: in sync ●");
        return Ok(());
    }

    let diff = TextDiff::from_lines(&disk_content, &store_content);
    println!("diff {rs}/{repo} (disk → store):");
    for change in diff.iter_all_changes() {
        let prefix = match change.tag() {
            ChangeTag::Delete => "\x1b[31m-",
            ChangeTag::Insert => "\x1b[32m+",
            ChangeTag::Equal => " ",
        };
        print!("{prefix}{change}\x1b[0m");
    }
    Ok(())
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct Bundle {
    registry: store::registry::Registry,
    #[serde(default)]
    vars: IndexMap<String, IndexMap<String, String>>,
}

fn cmd_export_registry(config: &Path, output: Option<&str>) -> Result<()> {
    let reg = load_registry(config)?;
    let s = toml::to_string(&reg).context("serialize registry")?;
    match output {
        Some(path) => {
            std::fs::write(path, &s)?;
            println!("exported registry → {path}");
        }
        None => print!("{s}"),
    }
    Ok(())
}

fn cmd_merge_registry(config: &Path, file: &str, overwrite: bool) -> Result<()> {
    let incoming_str = std::fs::read_to_string(file)
        .with_context(|| format!("read {file}"))?;
    let incoming: store::registry::Registry = toml::from_str(&incoming_str).context("parse registry file")?;

    let mut reg = load_registry(config)?;
    let mut added_rs = 0usize;
    let mut added_repos = 0usize;

    for (name, entry) in &incoming.rs {
        if !reg.rs.contains_key(name) || overwrite {
            reg.rs.insert(name.clone(), entry.clone());
            added_rs += 1;
        } else {
            for (repo_name, repo_path) in &entry.repos {
                if !reg.rs[name].repos.contains_key(repo_name) || overwrite {
                    reg.rs.get_mut(name).unwrap().repos.insert(repo_name.clone(), repo_path.clone());
                    added_repos += 1;
                }
            }
        }
    }

    for (name, path) in &incoming.repos {
        if !reg.repos.contains_key(name) || overwrite {
            reg.repos.insert(name.clone(), path.clone());
            added_repos += 1;
        }
    }

    save_registry(config, &reg)?;
    println!("merged: {added_rs} RS, {added_repos} repos added/updated");
    Ok(())
}

fn cmd_export_bundle(config: &Path, output: Option<&str>) -> Result<()> {
    let registry = load_registry(config)?;
    let store = load_store(config)?;
    let mut vars: IndexMap<String, IndexMap<String, String>> = IndexMap::new();

    for (rs_name, repo_name) in store.list_all()? {
        let v = store.load(&rs_name, &repo_name)?;
        if !v.is_empty() {
            vars.insert(format!("{rs_name}/{repo_name}"), v);
        }
    }

    let bundle = Bundle { registry, vars };
    let s = toml::to_string(&bundle).context("serialize bundle")?;

    match output {
        Some(path) => {
            std::fs::write(path, &s)?;
            println!("exported bundle → {path}");
        }
        None => print!("{s}"),
    }
    Ok(())
}

fn cmd_import_bundle(config: &Path, file: &str, overwrite: bool) -> Result<()> {
    let s = std::fs::read_to_string(file).with_context(|| format!("read {file}"))?;
    let bundle: Bundle = toml::from_str(&s).context("parse bundle")?;

    let mut reg = load_registry(config)?;

    for (name, entry) in &bundle.registry.rs {
        if !reg.rs.contains_key(name) || overwrite {
            reg.rs.insert(name.clone(), entry.clone());
        } else {
            for (repo_name, repo_path) in &entry.repos {
                if !reg.rs[name].repos.contains_key(repo_name) || overwrite {
                    reg.rs.get_mut(name).unwrap().repos.insert(repo_name.clone(), repo_path.clone());
                }
            }
        }
    }
    for (name, path) in &bundle.registry.repos {
        if !reg.repos.contains_key(name) || overwrite {
            reg.repos.insert(name.clone(), path.clone());
        }
    }
    save_registry(config, &reg)?;

    let store = load_store(config)?;
    let mut imported_vars = 0usize;
    for (key, v) in &bundle.vars {
        let (rs, repo) = key.split_once('/').with_context(|| format!("invalid key in bundle: {key}"))?;
        store.save(rs, repo, v)?;
        imported_vars += v.len();
    }

    println!(
        "imported: {} RS, {} repos, {} vars",
        bundle.registry.rs.len(),
        bundle.vars.len(),
        imported_vars
    );
    Ok(())
}

fn cmd_dump(config: &Path, rs: &str, repo: &str) -> Result<()> {
    let store = load_store(config)?;
    let vars = store.load(rs, repo)?;
    if vars.is_empty() {
        eprintln!("no vars for {rs}/{repo}");
        return Ok(());
    }
    print!("{}", env_file::serialize(&vars));
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = config_dir()?;

    match cli.command {
        None => {
            let store = load_store(&config)?;
            let registry = load_registry(&config)?;
            tui::run(store, registry, registry_path(&config))?;
        }
        Some(Command::Init) => cmd_init(&config)?,
        Some(Command::Rs { cmd }) => match cmd {
            cli::RsCommand::Add { name, ip } => cmd_rs_add(&config, &name, &ip)?,
            cli::RsCommand::List => cmd_rs_list(&config)?,
            cli::RsCommand::Remove { name } => cmd_rs_remove(&config, &name)?,
            cli::RsCommand::Search { query } => cmd_rs_search(&config, &query)?,
        },
        Some(Command::Repo { cmd }) => match cmd {
            RepoCommand::Add { name, path } => cmd_repo_add(&config, &name, &path)?,
            RepoCommand::List => cmd_repo_list(&config)?,
            RepoCommand::Remove { name } => cmd_repo_remove(&config, &name)?,
            RepoCommand::Update { name, path } => cmd_repo_update(&config, &name, &path)?,
        },
        Some(Command::Register { path, rs, name }) => {
            cmd_register(&config, &path, &rs, name.as_deref())?
        }
        Some(Command::Scan { dir, rs }) => cmd_scan(&config, &dir, &rs)?,
        Some(Command::List { target }) => match target {
            ListTarget::Rs => cmd_list_rs(&config)?,
            ListTarget::Repos { rs } => cmd_list_repos(&config, rs.as_deref())?,
            ListTarget::Vars { rs, repo, reveal } => cmd_list_vars(&config, &rs, &repo, reveal)?,
        },
        Some(Command::Set { rs, repo, pair }) => cmd_set(&config, &rs, &repo, &pair)?,
        Some(Command::Get { rs, repo, key, reveal }) => {
            cmd_get(&config, &rs, &repo, &key, reveal)?
        }
        Some(Command::Delete { rs, repo, key }) => cmd_delete(&config, &rs, &repo, &key)?,
        Some(Command::Import { rs, repo, env_file }) => {
            cmd_import(&config, &rs, &repo, &env_file)?
        }
        Some(Command::Edit { rs, repo }) => cmd_edit(&config, &rs, &repo)?,
        Some(Command::Apply { rs: Some(rs), repo: Some(repo), .. }) => {
            cmd_apply(&config, &rs, &repo)?
        }
        Some(Command::Apply { all: true, .. }) => cmd_apply_all(&config)?,
        Some(Command::Apply { .. }) => {
            bail!("specify RS and REPO, or --all");
        }
        Some(Command::Diff { rs, repo }) => cmd_diff(&config, &rs, &repo)?,
        Some(Command::Export { target }) => match target {
            ExportTarget::Registry { output } => {
                cmd_export_registry(&config, output.as_deref())?
            }
            ExportTarget::Bundle { output } => {
                cmd_export_bundle(&config, output.as_deref())?
            }
        },
        Some(Command::MergeRegistry { file, overwrite }) => {
            cmd_merge_registry(&config, &file, overwrite)?
        }
        Some(Command::Dump { rs, repo }) => cmd_dump(&config, &rs, &repo)?,
        Some(Command::ImportBundle { file, overwrite }) => {
            cmd_import_bundle(&config, &file, overwrite)?
        }
    }

    Ok(())
}
