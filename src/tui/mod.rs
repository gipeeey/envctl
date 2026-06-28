use std::{collections::HashMap, io, path::PathBuf, time::{Duration, Instant}};

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use indexmap::IndexMap;
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use similar::ChangeTag;

use crate::{
    env_file,
    store::{Store, registry::Registry},
};

#[derive(PartialEq, Clone, Copy)]
enum View {
    Menu,
    EnvManager,
    RsManager,
    RepoManager,
}

#[derive(PartialEq, Clone, Copy)]
enum Pane {
    Rs,
    Repos,
    Vars,
}

#[derive(PartialEq)]
enum Mode {
    Normal,
    Search,
    SearchRs,
}

enum Screen {
    Main,
    Diff {
        rs: String,
        repo: String,
        lines: Vec<(ChangeTag, String)>,
    },
}

#[derive(Clone, Copy, PartialEq)]
enum Sync {
    Synced,
    Unsynced,
}

#[derive(PartialEq, Clone, Copy)]
enum RsFormField {
    Name,
    Ip,
}

struct RsForm {
    name: String,
    ip: String,
    focus: RsFormField,
}

impl RsForm {
    fn new() -> Self {
        Self { name: String::new(), ip: String::new(), focus: RsFormField::Name }
    }
}

#[derive(PartialEq, Clone, Copy)]
enum RepoFormField {
    Name,
    Path,
}

#[derive(PartialEq, Clone, Copy)]
enum VarFormField {
    Key,
    Value,
}

struct VarForm {
    key: String,
    value: String,
    focus: VarFormField,
}

impl VarForm {
    fn new() -> Self {
        Self { key: String::new(), value: String::new(), focus: VarFormField::Key }
    }
}

struct RepoForm {
    name: String,
    path: String,
    focus: RepoFormField,
    editing: Option<String>,
}

impl RepoForm {
    fn new() -> Self {
        Self { name: String::new(), path: String::new(), focus: RepoFormField::Name, editing: None }
    }

    fn edit(name: &str, path: &str) -> Self {
        Self {
            name: name.to_string(),
            path: path.to_string(),
            focus: RepoFormField::Path,
            editing: Some(name.to_string()),
        }
    }
}

const MENU_ITEMS: &[(&str, &str)] = &[
    ("Env Manager", "browse repos & env vars"),
    ("RS Manager", "add / remove RS clients"),
    ("Repo Manager", "add / remove global repos"),
];

struct App {
    view: View,
    menu_state: ListState,

    all_rs: Vec<String>,
    displayed_rs: Vec<String>,
    rs_search: String,
    rs_state: ListState,
    repos_for_rs: Vec<(String, PathBuf)>,
    global_repo_count: usize,
    repo_state: ListState,
    vars: IndexMap<String, String>,
    vars_scroll: usize,
    sync: HashMap<(String, String), Sync>,
    focus: Pane,
    reveal: bool,
    mode: Mode,
    search: String,
    screen: Screen,

    rs_mgr_state: ListState,
    rs_form: Option<RsForm>,

    repo_mgr_state: ListState,
    repo_form: Option<RepoForm>,

    var_form: Option<VarForm>,

    status: String,
    notification_until: Option<Instant>,
    needs_clear: bool,
    quit: bool,
    store: Store,
    registry: Registry,
    registry_path: PathBuf,
}

impl App {
    fn new(store: Store, registry: Registry, registry_path: PathBuf) -> Self {
        let all_rs: Vec<String> = registry.rs.keys().cloned().collect();
        let mut rs_state = ListState::default();
        if !all_rs.is_empty() {
            rs_state.select(Some(0));
        }
        let mut menu_state = ListState::default();
        menu_state.select(Some(0));
        let mut rs_mgr_state = ListState::default();
        if !all_rs.is_empty() {
            rs_mgr_state.select(Some(0));
        }

        let all_global_repos: Vec<String> = registry.repos.keys().cloned().collect();
        let mut repo_mgr_state = ListState::default();
        if !all_global_repos.is_empty() {
            repo_mgr_state.select(Some(0));
        }

        let displayed_rs = all_rs.clone();

        let mut app = Self {
            view: View::Menu,
            menu_state,
            displayed_rs,
            all_rs,
            rs_search: String::new(),
            rs_state,
            repos_for_rs: Vec::new(),
            global_repo_count: 0,
            repo_state: ListState::default(),
            vars: IndexMap::new(),
            vars_scroll: 0,
            sync: HashMap::new(),
            focus: Pane::Rs,
            reveal: false,
            mode: Mode::Normal,
            search: String::new(),
            screen: Screen::Main,
            rs_mgr_state,
            rs_form: None,
            repo_mgr_state,
            repo_form: None,
            var_form: None,
            status: String::new(),
            notification_until: None,
            needs_clear: false,
            quit: false,
            store,
            registry,
            registry_path,
        };

        app.reload_repos();
        app.compute_all_sync();
        app.load_vars();
        app
    }

    fn notify(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
        self.notification_until = Some(Instant::now() + Duration::from_secs(3));
    }

    fn clear_status(&mut self) {
        self.status.clear();
        self.notification_until = None;
    }

    fn selected_rs(&self) -> Option<&str> {
        self.displayed_rs.get(self.rs_state.selected()?).map(String::as_str)
    }

    fn reload_rs_list(&mut self) {
        let q = self.rs_search.to_lowercase();
        self.displayed_rs = if q.is_empty() {
            self.all_rs.clone()
        } else {
            self.all_rs.iter().filter(|name| {
                name.to_lowercase().contains(&q)
                    || self.registry.rs.get(*name)
                        .map(|e| e.ip.to_lowercase().contains(&q))
                        .unwrap_or(false)
            }).cloned().collect()
        };
        if self.displayed_rs.is_empty() {
            self.rs_state.select(None);
        } else {
            let cur = self.rs_state.selected().unwrap_or(0);
            self.rs_state.select(Some(cur.min(self.displayed_rs.len() - 1)));
        }
    }

    fn selected_repo(&self) -> Option<&(String, PathBuf)> {
        self.repos_for_rs.get(self.repo_state.selected()?)
    }

    fn selected_rs_in_mgr(&self) -> Option<&str> {
        self.all_rs.get(self.rs_mgr_state.selected()?).map(String::as_str)
    }

    fn reload_repos(&mut self) {
        if self.selected_rs().is_none() {
            self.repos_for_rs.clear();
            self.repo_state.select(None);
            return;
        }
        let rs_name = self.selected_rs().unwrap().to_string();
        let q = self.search.to_lowercase();

        let mut seen = std::collections::HashSet::new();
        let mut repos: Vec<(String, PathBuf)> = Vec::new();

        for (name, path) in &self.registry.repos {
            if q.is_empty() || name.to_lowercase().contains(&q) {
                if seen.insert(name.clone()) {
                    repos.push((name.clone(), PathBuf::from(path)));
                }
            }
        }
        let global_count = repos.len();
        if let Some(entry) = self.registry.rs.get(&rs_name) {
            for (name, path) in &entry.repos {
                if q.is_empty() || name.to_lowercase().contains(&q) {
                    if seen.insert(name.clone()) {
                        repos.push((name.clone(), PathBuf::from(path)));
                    }
                }
            }
        }

        self.global_repo_count = global_count;
        self.repos_for_rs = repos;
        if self.repos_for_rs.is_empty() {
            self.repo_state.select(None);
        } else {
            let cur = self.repo_state.selected().unwrap_or(0);
            self.repo_state.select(Some(cur.min(self.repos_for_rs.len() - 1)));
        }
    }

    fn load_vars(&mut self) {
        let rs = match self.selected_rs() {
            Some(n) => n.to_string(),
            None => { self.vars.clear(); return; }
        };
        let repo = match self.selected_repo() {
            Some((n, _)) => n.clone(),
            None => { self.vars.clear(); return; }
        };
        self.vars = self.store.load(&rs, &repo).unwrap_or_default();
        self.vars_scroll = 0;
    }

    fn compute_sync_for(&self, rs: &str, repo: &str, path: &PathBuf) -> Sync {
        let vars = self.store.load(rs, repo).unwrap_or_default();
        if vars.is_empty() { return Sync::Unsynced; }
        let store_str = env_file::serialize(&vars);
        let disk_str = std::fs::read_to_string(path.join(".env")).unwrap_or_default();
        if store_str == disk_str { Sync::Synced } else { Sync::Unsynced }
    }

    /// Mark `rs` as the active RS for `repo` and deactivate every other RS.
    /// Global repos share a single `.env` on disk, so only one RS can be the
    /// active deployment for a repo at a time.
    fn mark_active(&mut self, rs: &str, repo: &str) {
        self.sync.insert((rs.to_string(), repo.to_string()), Sync::Synced);
        if self.registry.repos.contains_key(repo) {
            for other in self.all_rs.clone() {
                if other != rs {
                    self.sync.insert((other, repo.to_string()), Sync::Unsynced);
                }
            }
        }
    }

    fn compute_all_sync(&mut self) {
        // Global repos share one `.env` on disk → at most ONE RS can be active
        // per repo. First RS whose stored vars match disk wins; rest forced off.
        let global: Vec<(String, String)> = self
            .registry
            .repos
            .iter()
            .map(|(n, p)| (n.clone(), p.clone()))
            .collect();
        for (repo_name, path_str) in &global {
            let path = PathBuf::from(path_str);
            let mut active_taken = false;
            for rs_name in self.all_rs.clone() {
                let s = if !active_taken
                    && matches!(self.compute_sync_for(&rs_name, repo_name, &path), Sync::Synced)
                {
                    active_taken = true;
                    Sync::Synced
                } else {
                    Sync::Unsynced
                };
                self.sync.insert((rs_name, repo_name.clone()), s);
            }
        }
        // RS-specific repos own their path → not shared, computed independently.
        let rs_specific: Vec<(String, String, String)> = self
            .registry
            .rs
            .iter()
            .flat_map(|(rs_name, entry)| {
                entry
                    .repos
                    .iter()
                    .map(move |(repo_name, path_str)| {
                        (rs_name.clone(), repo_name.clone(), path_str.clone())
                    })
            })
            .collect();
        for (rs_name, repo_name, path_str) in &rs_specific {
            let path = PathBuf::from(path_str);
            let s = self.compute_sync_for(rs_name, repo_name, &path);
            self.sync.insert((rs_name.clone(), repo_name.clone()), s);
        }
    }

    fn apply_selected(&mut self) {
        let rs = match self.selected_rs() { Some(n) => n.to_string(), None => return };
        let Some((repo, path)) = self.selected_repo().cloned() else { return };
        let vars = self.store.load(&rs, &repo).unwrap_or_default();
        let content = env_file::serialize(&vars);
        match std::fs::write(path.join(".env"), &content) {
            Ok(_) => {
                self.mark_active(&rs, &repo);
                self.notify(format!("✓ applied {} vars → {rs}/{repo}", vars.len()));
            }
            Err(e) => self.notify(format!("✗ {e}")),
        }
    }

    fn apply_all(&mut self) {
        let rs = match self.selected_rs() { Some(n) => n.to_string(), None => return };
        let repos = self.repos_for_rs.clone();
        let mut ok = 0usize;
        let mut fail = 0usize;
        for (repo, path) in &repos {
            let vars = self.store.load(&rs, repo).unwrap_or_default();
            let content = env_file::serialize(&vars);
            match std::fs::write(path.join(".env"), &content) {
                Ok(_) => {
                    self.mark_active(&rs, repo);
                    ok += 1;
                }
                Err(_) => fail += 1,
            }
        }
        let msg = if fail == 0 {
            format!("✓ applied all {ok} repos → {rs}")
        } else {
            format!("✓ {ok} applied, ✗ {fail} failed → {rs}")
        };
        self.notify(msg);
    }

    fn show_diff(&mut self) {
        let rs = match self.selected_rs() { Some(n) => n.to_string(), None => return };
        let Some((repo, path)) = self.selected_repo().cloned() else { return };
        let vars = self.store.load(&rs, &repo).unwrap_or_default();
        let store_str = env_file::serialize(&vars);
        let disk_str = std::fs::read_to_string(path.join(".env")).unwrap_or_default();
        let diff = similar::TextDiff::from_lines(&disk_str, &store_str);
        let lines = diff.iter_all_changes().map(|c| (c.tag(), c.to_string())).collect();
        self.screen = Screen::Diff { rs, repo, lines };
    }

    fn apply_search(&mut self) {
        self.reload_repos();
        self.load_vars();
    }

    fn nav_rs(&mut self, delta: i32) {
        if self.displayed_rs.is_empty() { return; }
        let cur = self.rs_state.selected().unwrap_or(0) as i32;
        let next = (cur + delta).clamp(0, self.displayed_rs.len() as i32 - 1) as usize;
        self.rs_state.select(Some(next));
        self.reload_repos();
        self.load_vars();
    }

    fn nav_repo(&mut self, delta: i32) {
        if self.repos_for_rs.is_empty() { return; }
        let cur = self.repo_state.selected().unwrap_or(0) as i32;
        let next = (cur + delta).clamp(0, self.repos_for_rs.len() as i32 - 1) as usize;
        self.repo_state.select(Some(next));
        self.load_vars();
    }

    fn open_editor(&mut self) -> Result<()> {
        let rs = match self.selected_rs() { Some(n) => n.to_string(), None => return Ok(()) };
        let Some((repo, _)) = self.selected_repo().cloned() else { return Ok(()) };
        let vars = self.store.load(&rs, &repo).unwrap_or_default();
        let content = env_file::serialize(&vars);

        let mut tmp = tempfile::Builder::new()
            .prefix(&format!("envctl-{rs}-{repo}-"))
            .suffix(".env")
            .tempfile()?;
        use std::io::Write as _;
        tmp.write_all(content.as_bytes())?;
        let tmp_path = tmp.path().to_path_buf();

        disable_raw_mode()?;
        execute!(io::stdout(), LeaveAlternateScreen)?;
        let editor = std::env::var("EDITOR")
            .or_else(|_| std::env::var("VISUAL"))
            .unwrap_or_else(|_| if cfg!(windows) { "notepad".to_string() } else { "nano".to_string() });
        let _ = std::process::Command::new(&editor).arg(&tmp_path).status();
        let updated = std::fs::read_to_string(&tmp_path).unwrap_or_default();
        let new_vars = env_file::parse(&updated);
        let count = new_vars.len();
        self.store.save(&rs, &repo, &new_vars)?;
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen)?;
        self.needs_clear = true;

        self.load_vars();
        if let Some((rep, path)) = self.selected_repo().cloned() {
            let s = self.compute_sync_for(&rs, &rep, &path);
            self.sync.insert((rs.clone(), rep), s);
        }
        self.notify(format!("saved {count} vars for {rs}/{repo}"));
        Ok(())
    }

    fn rs_mgr_add_submit(&mut self) -> Result<()> {
        let form = match self.rs_form.take() { Some(f) => f, None => return Ok(()) };
        let name = form.name.trim().to_string();
        let ip = form.ip.trim().to_string();
        if name.is_empty() || ip.is_empty() {
            self.notify("name and IP required");
            return Ok(());
        }
        if self.registry.rs.contains_key(&name) {
            self.notify(format!("RS already exists: {name}"));
            return Ok(());
        }
        self.registry.add_rs(name.clone(), ip.clone());
        self.registry.save(&self.registry_path)?;
        self.all_rs = self.registry.rs.keys().cloned().collect();
        self.reload_rs_list();
        let idx = self.all_rs.iter().position(|n| n == &name).unwrap_or(0);
        self.rs_mgr_state.select(Some(idx));
        let disp_idx = self.displayed_rs.iter().position(|n| n == &name).unwrap_or(0);
        self.rs_state.select(Some(disp_idx));
        self.reload_repos();
        self.notify(format!("added RS: {name} ({ip})"));
        Ok(())
    }

    fn rs_mgr_delete(&mut self) -> Result<()> {
        let name = match self.selected_rs_in_mgr() { Some(n) => n.to_string(), None => return Ok(()) };
        self.registry.rs.shift_remove(&name);
        self.registry.save(&self.registry_path)?;
        self.all_rs = self.registry.rs.keys().cloned().collect();
        self.reload_rs_list();
        let mgr_sel = if self.all_rs.is_empty() {
            None
        } else {
            let cur = self.rs_mgr_state.selected().unwrap_or(0);
            Some(cur.min(self.all_rs.len() - 1))
        };
        self.rs_mgr_state.select(mgr_sel);
        let disp_sel = if self.displayed_rs.is_empty() {
            None
        } else {
            let cur = self.rs_state.selected().unwrap_or(0);
            Some(cur.min(self.displayed_rs.len() - 1))
        };
        self.rs_state.select(disp_sel);
        self.reload_repos();
        self.load_vars();
        self.notify(format!("deleted RS: {name}"));
        Ok(())
    }

    fn repo_mgr_add_submit(&mut self) -> Result<()> {
        let form = match self.repo_form.take() { Some(f) => f, None => return Ok(()) };
        let name = form.name.trim().to_string();
        let path_str = form.path.trim().to_string();
        if name.is_empty() || path_str.is_empty() {
            self.notify("name and path required");
            return Ok(());
        }
        let abs = match std::fs::canonicalize(&path_str) {
            Ok(p) => p,
            Err(e) => {
                self.notify(format!("invalid path: {e}"));
                return Ok(());
            }
        };
        if let Some(orig) = form.editing {
            self.registry.repos.shift_remove(&orig);
        } else if self.registry.repos.contains_key(&name) {
            self.notify(format!("repo already exists: {name}"));
            return Ok(());
        }
        self.registry.add_repo(name.clone(), abs.clone());
        self.registry.save(&self.registry_path)?;
        let all_repos: Vec<String> = self.registry.repos.keys().cloned().collect();
        let idx = all_repos.iter().position(|n| n == &name).unwrap_or(0);
        self.repo_mgr_state.select(Some(idx));
        self.reload_repos();
        self.compute_all_sync();
        self.notify(format!("saved repo: {name} → {}", abs.display()));
        Ok(())
    }

    fn var_form_submit(&mut self) -> Result<()> {
        let form = match self.var_form.take() { Some(f) => f, None => return Ok(()) };
        let key = form.key.trim().to_string();
        let value = form.value.trim().to_string();
        if key.is_empty() {
            self.notify("key required");
            return Ok(());
        }
        let rs = match self.selected_rs() { Some(n) => n.to_string(), None => return Ok(()) };
        let repo = match self.selected_repo() { Some((n, _)) => n.clone(), None => return Ok(()) };
        self.vars.insert(key.clone(), value);
        self.store.save(&rs, &repo, &self.vars)?;
        if let Some((rep, path)) = self.selected_repo().cloned() {
            let s = self.compute_sync_for(&rs, &rep, &path);
            self.sync.insert((rs.clone(), rep), s);
        }
        self.notify(format!("added var: {key}"));
        Ok(())
    }

    fn repo_mgr_delete(&mut self) -> Result<()> {
        let all_repos: Vec<String> = self.registry.repos.keys().cloned().collect();
        let name = match self.repo_mgr_state.selected().and_then(|i| all_repos.get(i)) {
            Some(n) => n.clone(),
            None => return Ok(()),
        };
        self.registry.remove_repo(&name);
        self.registry.save(&self.registry_path)?;
        let new_len = self.registry.repos.len();
        let new_sel = if new_len == 0 { None } else {
            let cur = self.repo_mgr_state.selected().unwrap_or(0);
            Some(cur.min(new_len - 1))
        };
        self.repo_mgr_state.select(new_sel);
        self.reload_repos();
        self.compute_all_sync();
        self.notify(format!("deleted repo: {name}"));
        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return Ok(());
        }
        match self.view {
            View::Menu => self.handle_key_menu(key),
            View::EnvManager => self.handle_key_env(key),
            View::RsManager => self.handle_key_rs_mgr(key),
            View::RepoManager => self.handle_key_repo_mgr(key),
        }
    }

    fn handle_key_menu(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Char('j') | KeyCode::Down => {
                let cur = self.menu_state.selected().unwrap_or(0);
                self.menu_state.select(Some((cur + 1).min(MENU_ITEMS.len() - 1)));
            }
            KeyCode::Char('k') | KeyCode::Up => {
                let cur = self.menu_state.selected().unwrap_or(0);
                self.menu_state.select(Some(cur.saturating_sub(1)));
            }
            KeyCode::Enter | KeyCode::Char(' ') => match self.menu_state.selected() {
                Some(0) => self.view = View::EnvManager,
                Some(1) => self.view = View::RsManager,
                Some(2) => self.view = View::RepoManager,
                _ => {}
            },
            KeyCode::Char('1') => self.view = View::EnvManager,
            KeyCode::Char('2') => self.view = View::RsManager,
            KeyCode::Char('3') => self.view = View::RepoManager,
            _ => {}
        }
        Ok(())
    }

    fn handle_key_env(&mut self, key: KeyEvent) -> Result<()> {
        if matches!(self.screen, Screen::Diff { .. }) {
            self.screen = Screen::Main;
            return Ok(());
        }

        if self.var_form.is_some() {
            match key.code {
                KeyCode::Esc => { self.var_form = None; self.notify("cancelled"); }
                KeyCode::Enter => {
                    let advance = self.var_form.as_ref()
                        .map(|f| f.focus == VarFormField::Key && !f.key.is_empty())
                        .unwrap_or(false);
                    if advance {
                        self.var_form.as_mut().unwrap().focus = VarFormField::Value;
                    } else {
                        self.var_form_submit()?;
                    }
                }
                KeyCode::Tab => {
                    if let Some(f) = &mut self.var_form {
                        f.focus = match f.focus {
                            VarFormField::Key => VarFormField::Value,
                            VarFormField::Value => VarFormField::Key,
                        };
                    }
                }
                KeyCode::Backspace => {
                    if let Some(f) = &mut self.var_form {
                        match f.focus {
                            VarFormField::Key => { f.key.pop(); }
                            VarFormField::Value => { f.value.pop(); }
                        }
                    }
                }
                KeyCode::Char(c) => {
                    if let Some(f) = &mut self.var_form {
                        match f.focus {
                            VarFormField::Key => f.key.push(c),
                            VarFormField::Value => f.value.push(c),
                        }
                    }
                }
                _ => {}
            }
            return Ok(());
        }

        if self.mode == Mode::SearchRs {
            match key.code {
                KeyCode::Esc | KeyCode::Enter => {
                    self.mode = Mode::Normal;
                    self.reload_repos();
                    self.load_vars();
                }
                KeyCode::Backspace => {
                    self.rs_search.pop();
                    self.reload_rs_list();
                    self.reload_repos();
                    self.load_vars();
                }
                KeyCode::Char(c) => {
                    self.rs_search.push(c);
                    self.reload_rs_list();
                    self.reload_repos();
                    self.load_vars();
                }
                _ => {}
            }
            return Ok(());
        }

        if self.mode == Mode::Search {
            match key.code {
                KeyCode::Esc | KeyCode::Enter => self.mode = Mode::Normal,
                KeyCode::Backspace => { self.search.pop(); self.apply_search(); }
                KeyCode::Char(c) => { self.search.push(c); self.apply_search(); }
                _ => {}
            }
            return Ok(());
        }

        match self.focus {
            Pane::Rs => match key.code {
                KeyCode::Char('q') => self.quit = true,
                KeyCode::Esc => {
                    if self.rs_search.is_empty() {
                        self.view = View::Menu;
                        self.clear_status();
                    } else {
                        self.rs_search.clear();
                        self.reload_rs_list();
                        self.reload_repos();
                        self.load_vars();
                    }
                }
                KeyCode::Char('j') | KeyCode::Down => self.nav_rs(1),
                KeyCode::Char('k') | KeyCode::Up => self.nav_rs(-1),
                KeyCode::Tab | KeyCode::Right => {
                    if !self.repos_for_rs.is_empty() { self.focus = Pane::Repos; }
                }
                KeyCode::Char('/') => {
                    self.mode = Mode::SearchRs;
                    self.rs_search.clear();
                    self.reload_rs_list();
                }
                KeyCode::Char('r') => {
                    self.compute_all_sync();
                    self.notify("refreshed sync status");
                }
                _ => {}
            },
            Pane::Repos => match key.code {
                KeyCode::Char('q') | KeyCode::Left => self.focus = Pane::Rs,
                KeyCode::Esc => {
                    if self.search.is_empty() { self.focus = Pane::Rs; }
                    else { self.search.clear(); self.apply_search(); }
                }
                KeyCode::Char('j') | KeyCode::Down => self.nav_repo(1),
                KeyCode::Char('k') | KeyCode::Up => self.nav_repo(-1),
                KeyCode::Tab | KeyCode::Right => {
                    if self.selected_repo().is_some() { self.focus = Pane::Vars; }
                }
                KeyCode::Char('/') => { self.mode = Mode::Search; self.search.clear(); self.apply_search(); }
                KeyCode::Char('a') => self.apply_selected(),
                        KeyCode::Char('A') => self.apply_all(),
                KeyCode::Char('e') => self.open_editor()?,
                KeyCode::Char('d') => self.show_diff(),
                KeyCode::Char('v') => {
                    self.reveal = !self.reveal;
                    self.notify(if self.reveal { "values revealed (v to mask)" } else { "values masked" });
                }
                KeyCode::Char('r') => {
                    self.compute_all_sync();
                    self.notify("refreshed sync status");
                }
                _ => {}
            },
            Pane::Vars => match key.code {
                KeyCode::Char('q') | KeyCode::Esc | KeyCode::Left | KeyCode::Tab => {
                    self.focus = Pane::Repos;
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    if self.vars_scroll + 1 < self.vars.len() { self.vars_scroll += 1; }
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.vars_scroll = self.vars_scroll.saturating_sub(1);
                }
                KeyCode::Char('e') => self.open_editor()?,
                KeyCode::Char('v') => self.reveal = !self.reveal,
                KeyCode::Char('n') => {
                    if self.selected_repo().is_some() {
                        self.var_form = Some(VarForm::new());
                        self.clear_status();
                    }
                }
                _ => {}
            },
        }
        Ok(())
    }

    fn handle_key_rs_mgr(&mut self, key: KeyEvent) -> Result<()> {
        if self.rs_form.is_some() {
            match key.code {
                KeyCode::Esc => { self.rs_form = None; self.notify("cancelled"); }
                KeyCode::Enter => {
                    let advance = self.rs_form.as_ref()
                        .map(|f| f.focus == RsFormField::Name && !f.name.is_empty())
                        .unwrap_or(false);
                    if advance {
                        self.rs_form.as_mut().unwrap().focus = RsFormField::Ip;
                    } else {
                        self.rs_mgr_add_submit()?;
                    }
                }
                KeyCode::Tab => {
                    if let Some(f) = &mut self.rs_form {
                        f.focus = match f.focus {
                            RsFormField::Name => RsFormField::Ip,
                            RsFormField::Ip => RsFormField::Name,
                        };
                    }
                }
                KeyCode::Backspace => {
                    if let Some(f) = &mut self.rs_form {
                        match f.focus {
                            RsFormField::Name => { f.name.pop(); }
                            RsFormField::Ip => { f.ip.pop(); }
                        }
                    }
                }
                KeyCode::Char(c) => {
                    if let Some(f) = &mut self.rs_form {
                        match f.focus {
                            RsFormField::Name => f.name.push(c),
                            RsFormField::Ip => f.ip.push(c),
                        }
                    }
                }
                _ => {}
            }
            return Ok(());
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => { self.view = View::Menu; self.clear_status(); }
            KeyCode::Char('j') | KeyCode::Down => {
                if !self.all_rs.is_empty() {
                    let cur = self.rs_mgr_state.selected().unwrap_or(0);
                    self.rs_mgr_state.select(Some((cur + 1).min(self.all_rs.len() - 1)));
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if !self.all_rs.is_empty() {
                    let cur = self.rs_mgr_state.selected().unwrap_or(0);
                    self.rs_mgr_state.select(Some(cur.saturating_sub(1)));
                }
            }
            KeyCode::Char('a') => { self.rs_form = Some(RsForm::new()); self.clear_status(); }
            KeyCode::Char('d') => self.rs_mgr_delete()?,
            KeyCode::Enter => {
                if let Some(name) = self.selected_rs_in_mgr().map(str::to_string) {
                    let idx = self.all_rs.iter().position(|n| n == &name).unwrap_or(0);
                    self.rs_state.select(Some(idx));
                    self.focus = Pane::Rs;
                    self.reload_repos();
                    self.load_vars();
                    self.view = View::EnvManager;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_key_repo_mgr(&mut self, key: KeyEvent) -> Result<()> {
        if self.repo_form.is_some() {
            match key.code {
                KeyCode::Esc => { self.repo_form = None; self.notify("cancelled"); }
                KeyCode::Enter => {
                    let advance = self.repo_form.as_ref()
                        .map(|f| f.focus == RepoFormField::Name && !f.name.is_empty())
                        .unwrap_or(false);
                    if advance {
                        self.repo_form.as_mut().unwrap().focus = RepoFormField::Path;
                    } else {
                        self.repo_mgr_add_submit()?;
                    }
                }
                KeyCode::Tab => {
                    if let Some(f) = &mut self.repo_form {
                        f.focus = match f.focus {
                            RepoFormField::Name => RepoFormField::Path,
                            RepoFormField::Path => RepoFormField::Name,
                        };
                    }
                }
                KeyCode::Backspace => {
                    if let Some(f) = &mut self.repo_form {
                        match f.focus {
                            RepoFormField::Name => { f.name.pop(); }
                            RepoFormField::Path => { f.path.pop(); }
                        }
                    }
                }
                KeyCode::Char(c) => {
                    if let Some(f) = &mut self.repo_form {
                        match f.focus {
                            RepoFormField::Name => f.name.push(c),
                            RepoFormField::Path => f.path.push(c),
                        }
                    }
                }
                _ => {}
            }
            return Ok(());
        }

        let repo_count = self.registry.repos.len();
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => { self.view = View::Menu; self.clear_status(); }
            KeyCode::Char('j') | KeyCode::Down => {
                if repo_count > 0 {
                    let cur = self.repo_mgr_state.selected().unwrap_or(0);
                    self.repo_mgr_state.select(Some((cur + 1).min(repo_count - 1)));
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if repo_count > 0 {
                    let cur = self.repo_mgr_state.selected().unwrap_or(0);
                    self.repo_mgr_state.select(Some(cur.saturating_sub(1)));
                }
            }
            KeyCode::Char('a') => { self.repo_form = Some(RepoForm::new()); self.clear_status(); }
            KeyCode::Char('e') => {
                let repos: Vec<(String, String)> = self.registry.repos.iter()
                    .map(|(n, p)| (n.clone(), p.clone()))
                    .collect();
                if let Some(i) = self.repo_mgr_state.selected() {
                    if let Some((name, path)) = repos.get(i) {
                        self.repo_form = Some(RepoForm::edit(name, path));
                        self.clear_status();
                    }
                }
            }
            KeyCode::Char('d') => self.repo_mgr_delete()?,
            _ => {}
        }
        Ok(())
    }
}

fn mask(v: &str) -> String {
    if v.len() <= 4 { "••••".into() } else { format!("{}••••", &v[..2]) }
}

fn border_style(focused: bool) -> Style {
    if focused { Style::default().fg(Color::Rgb(218, 119, 86)) } else { Style::default().fg(Color::DarkGray) }
}

fn draw(f: &mut Frame, app: &mut App) {
    match app.view {
        View::Menu => draw_menu(f, app),
        View::EnvManager => draw_env_manager(f, app),
        View::RsManager => draw_rs_manager(f, app),
        View::RepoManager => draw_repo_manager(f, app),
    }
}

fn draw_menu(f: &mut Frame, app: &mut App) {
    let area = f.area();

    let block = Block::default()
        .title(" envctl ")
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    f.render_widget(block, area);

    let inner_w = area.width.saturating_sub(2);
    let inner_h = area.height.saturating_sub(2);
    let menu_h = MENU_ITEMS.len() as u16 * 2 + 2;
    let top = area.y + 1 + inner_h.saturating_sub(menu_h) / 2;

    let items: Vec<ListItem> = MENU_ITEMS
        .iter()
        .enumerate()
        .map(|(i, (label, desc))| {
            let num = format!("[{}] ", i + 1);
            ListItem::new(Line::from(vec![
                Span::styled(num, Style::default().fg(Color::DarkGray)),
                Span::styled(*label, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
                Span::styled(format!("  — {desc}"), Style::default().fg(Color::DarkGray)),
            ]))
        })
        .collect();

    let menu_area = Rect {
        x: area.x + 1 + inner_w.saturating_sub(60) / 2,
        y: top,
        width: inner_w.min(60),
        height: menu_h,
    };

    let list = List::new(items)
        .highlight_style(
            Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    f.render_stateful_widget(list, menu_area, &mut app.menu_state);

    let hint = Paragraph::new("[j/k] nav  [enter] select  [q] quit")
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center);
    let hint_area = Rect { x: area.x + 1, y: area.y + area.height - 2, width: inner_w, height: 1 };
    f.render_widget(hint, hint_area);
}

fn draw_env_manager(f: &mut Frame, app: &mut App) {
    let area = f.area();

    if let Screen::Diff { rs, repo, lines } = &app.screen {
        let rs = rs.clone();
        let repo = repo.clone();
        let lines = lines.clone();
        draw_diff(f, area, &rs, &repo, &lines);
        return;
    }

    let vert = ratatui::layout::Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
    let horiz = ratatui::layout::Layout::horizontal([
        Constraint::Percentage(30),
        Constraint::Percentage(28),
        Constraint::Percentage(42),
    ])
    .split(vert[0]);

    draw_rs_pane(f, horiz[0], app);
    draw_repos(f, horiz[1], app);
    draw_vars(f, horiz[2], app);
    draw_statusbar(f, vert[1], app);
    if app.var_form.is_some() {
        draw_var_form(f, area, app);
    }
    if !app.status.is_empty() {
        draw_notification(f, area, &app.status.clone());
    }
}

fn draw_rs_pane(f: &mut Frame, area: Rect, app: &mut App) {
    let focused = app.focus == Pane::Rs;
    let title = match app.mode {
        Mode::SearchRs => format!(" ◈ RS / {} ", app.rs_search),
        _ if !app.rs_search.is_empty() => format!(
            " ◈ RS [{}/{}] / {} ",
            app.displayed_rs.len(), app.all_rs.len(), app.rs_search
        ),
        _ => format!(" ◈ RS [{}] ", app.all_rs.len()),
    };
    let block = Block::default().title(title).borders(Borders::ALL).border_style(border_style(focused));

    let items: Vec<ListItem> = app
        .displayed_rs
        .iter()
        .map(|name| {
            let ip = app.registry.rs.get(name).map(|e| e.ip.as_str()).unwrap_or("?");
            ListItem::new(Line::from(vec![
                Span::styled(name.as_str(), Style::default().fg(Color::White)),
                Span::styled(format!("  {ip}"), Style::default().fg(Color::DarkGray)),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD))
        .highlight_symbol("▶ ");

    f.render_stateful_widget(list, area, &mut app.rs_state);
}

fn draw_repos(f: &mut Frame, area: Rect, app: &mut App) {
    let focused = app.focus == Pane::Repos;
    let rs_name = app.selected_rs().unwrap_or("—").to_string();
    let total = app.registry.repos.len();
    let title = match &app.mode {
        Mode::Search => format!(" REPOS / {} ", app.search),
        _ if !app.search.is_empty() => format!(
            " REPOS [{}] {}/{} / {} ",
            rs_name,
            app.repos_for_rs.len(),
            total,
            app.search
        ),
        _ => format!(" REPOS [{}] {}/{} ", rs_name, app.repos_for_rs.len(), total),
    };

    let block = Block::default().title(title).borders(Borders::ALL).border_style(border_style(focused));

    let selected_idx = app.repo_state.selected();
    let items: Vec<ListItem> = app
        .repos_for_rs
        .iter()
        .enumerate()
        .map(|(i, (name, _))| {
            let rs = app.selected_rs().unwrap_or("").to_string();
            let (sym, sym_style) = match app.sync.get(&(rs, name.clone())) {
                Some(Sync::Synced) => ("● ", Style::default().fg(Color::Rgb(218, 119, 86))),
                _ => ("○ ", Style::default().fg(Color::Yellow)),
            };
            let name_fg = if selected_idx == Some(i) { Color::Black } else { Color::White };
            ListItem::new(Line::from(vec![
                Span::styled(sym, sym_style),
                Span::styled(name.as_str(), Style::default().fg(name_fg)),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().bg(Color::White).add_modifier(Modifier::BOLD))
        .highlight_symbol("▶ ");

    f.render_stateful_widget(list, area, &mut app.repo_state);
}


fn draw_vars(f: &mut Frame, area: Rect, app: &mut App) {
    let focused = app.focus == Pane::Vars;
    let rs_name = app.selected_rs().unwrap_or("—");
    let repo_name = app.selected_repo().map(|(n, _)| n.as_str()).unwrap_or("—");
    let title = format!(" VARS — {rs_name}/{repo_name} [{}] ", app.vars.len());

    let block = Block::default().title(title).borders(Borders::ALL).border_style(border_style(focused));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if app.vars.is_empty() {
        let msg = Paragraph::new("no vars  (use `envctl import` or press [e]dit)")
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(msg, inner);
        return;
    }

    let max_display = inner.height as usize;
    let lines: Vec<Line> = app
        .vars
        .iter()
        .skip(app.vars_scroll)
        .take(max_display)
        .map(|(k, v)| {
            let val = if app.reveal { v.clone() } else { mask(v) };
            Line::from(vec![
                Span::styled(k.as_str(), Style::default().fg(Color::Cyan)),
                Span::raw(" = "),
                Span::styled(val, Style::default().fg(Color::Yellow)),
            ])
        })
        .collect();

    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_statusbar(f: &mut Frame, area: Rect, app: &App) {
    let hint = match (&app.mode, app.focus) {
        (Mode::SearchRs, _) => " [enter/esc] done  type to filter RS by name or IP",
        (Mode::Search, _) => " [enter/esc] done searching",
        (Mode::Normal, Pane::Rs) => {
            " [j/k] nav  [/]search RS  [tab/→] repos  [r]efresh  [esc] menu  [q]uit"
        }
        (Mode::Normal, Pane::Repos) => {
            " [j/k] nav  [a]pply  [A]ll  [e]dit  [d]iff  [v]alues  [/]search  [r]efresh  [tab/→] vars  [esc/←] rs"
        }
        (Mode::Normal, Pane::Vars) => " [j/k] scroll  [n]ew var  [e]dit bulk  [v]alues  [tab/esc/←] back",
    };
    f.render_widget(
        Paragraph::new(hint).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

fn draw_notification(f: &mut Frame, area: Rect, msg: &str) {
    let is_err = msg.starts_with('✗');
    let (bg, fg) = if is_err {
        (Color::Red, Color::White)
    } else {
        (Color::Rgb(218, 119, 86), Color::Black)
    };
    let width = (msg.len() as u16 + 4).min(area.width.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + area.height / 4;
    let popup = Rect { x, y, width, height: 3 };
    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(format!(" {msg} "))
            .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(fg).bg(bg)))
            .style(Style::default().fg(fg).bg(bg))
            .alignment(Alignment::Center),
        popup,
    );
}

fn draw_rs_manager(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let vert = ratatui::layout::Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);

    let block = Block::default()
        .title(format!(" RS Manager [{}] ", app.all_rs.len()))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let items: Vec<ListItem> = app
        .all_rs
        .iter()
        .map(|name| {
            let entry = app.registry.rs.get(name);
            let ip = entry.map(|e| e.ip.as_str()).unwrap_or("?");
            let repos = entry.map(|e| e.repos.len()).unwrap_or(0);
            ListItem::new(Line::from(vec![
                Span::styled(name.as_str(), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
                Span::styled(format!("  {ip}"), Style::default().fg(Color::Cyan)),
                Span::styled(format!("  ({repos} repos)"), Style::default().fg(Color::DarkGray)),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD))
        .highlight_symbol("▶ ");

    f.render_stateful_widget(list, vert[0], &mut app.rs_mgr_state);

    let hint = if app.rs_form.is_some() {
        " [tab] next field  [enter] confirm  [esc] cancel"
    } else {
        " [j/k] nav  [a]dd RS  [d]elete  [enter] open in Env Manager  [esc/q] menu"
    };
    let status_text = if app.status.is_empty() {
        hint.to_string()
    } else {
        format!("  {}  │{}", app.status, hint)
    };
    f.render_widget(
        Paragraph::new(status_text).style(Style::default().fg(Color::DarkGray)),
        vert[1],
    );

    if app.rs_form.is_some() {
        draw_rs_form(f, area, app);
    }
}

fn draw_rs_form(f: &mut Frame, area: Rect, app: &App) {
    let form = app.rs_form.as_ref().unwrap();

    let w: u16 = 50;
    let h: u16 = 7;
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    let popup = Rect { x, y, width: w, height: h };

    f.render_widget(Clear, popup);

    let block = Block::default()
        .title(" Add RS ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let name_focused = form.focus == RsFormField::Name;
    let ip_focused = form.focus == RsFormField::Ip;

    let name_style = if name_focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let ip_style = if ip_focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let name_cursor = if name_focused { "█" } else { "" };
    let ip_cursor = if ip_focused { "█" } else { "" };

    let lines = vec![
        Line::from(vec![
            Span::styled("Name: ", name_style),
            Span::styled(&form.name, Style::default().fg(Color::White)),
            Span::styled(name_cursor, Style::default().fg(Color::Yellow)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("IP:   ", ip_style),
            Span::styled(&form.ip, Style::default().fg(Color::White)),
            Span::styled(ip_cursor, Style::default().fg(Color::Yellow)),
        ]),
    ];

    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_var_form(f: &mut Frame, area: Rect, app: &App) {
    let form = app.var_form.as_ref().unwrap();

    let w: u16 = 60;
    let h: u16 = 7;
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    let popup = Rect { x, y, width: w, height: h };

    f.render_widget(Clear, popup);

    let block = Block::default()
        .title(" Add Var ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let key_focused = form.focus == VarFormField::Key;
    let val_focused = form.focus == VarFormField::Value;

    let key_style = if key_focused { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };
    let val_style = if val_focused { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };

    let lines = vec![
        Line::from(vec![
            Span::styled("KEY:   ", key_style),
            Span::styled(&form.key, Style::default().fg(Color::Cyan)),
            Span::styled(if key_focused { "█" } else { "" }, Style::default().fg(Color::Yellow)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("VALUE: ", val_style),
            Span::styled(&form.value, Style::default().fg(Color::Yellow)),
            Span::styled(if val_focused { "█" } else { "" }, Style::default().fg(Color::Yellow)),
        ]),
    ];

    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_diff(f: &mut Frame, area: Rect, rs: &str, repo: &str, lines: &[(ChangeTag, String)]) {
    let block = Block::default()
        .title(format!(" DIFF — {rs}/{repo} (disk → store) "))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let hint = Line::from(Span::styled(" press any key to close", Style::default().fg(Color::DarkGray)));

    if lines.iter().all(|(t, _)| *t == ChangeTag::Equal) {
        let msg = vec![hint, Line::from(Span::styled(" in sync ●", Style::default().fg(Color::Green)))];
        f.render_widget(Paragraph::new(msg), inner);
        return;
    }

    let diff_lines: Vec<Line> = std::iter::once(hint)
        .chain(lines.iter().map(|(tag, content)| {
            let content = content.trim_end_matches('\n');
            match tag {
                ChangeTag::Insert => Line::from(Span::styled(format!("+{content}"), Style::default().fg(Color::Green))),
                ChangeTag::Delete => Line::from(Span::styled(format!("-{content}"), Style::default().fg(Color::Red))),
                ChangeTag::Equal => Line::from(Span::styled(format!(" {content}"), Style::default().fg(Color::DarkGray))),
            }
        }))
        .collect();

    f.render_widget(Paragraph::new(diff_lines).wrap(Wrap { trim: false }), inner);
}

fn draw_repo_manager(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let vert = ratatui::layout::Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);

    let repos: Vec<(String, String)> = app.registry.repos.iter()
        .map(|(n, p)| (n.clone(), p.clone()))
        .collect();

    let block = Block::default()
        .title(format!(" Repo Manager [{}] ", repos.len()))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let items: Vec<ListItem> = repos
        .iter()
        .map(|(name, path)| {
            ListItem::new(Line::from(vec![
                Span::styled(name.as_str(), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
                Span::styled(format!("  {path}"), Style::default().fg(Color::DarkGray)),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD))
        .highlight_symbol("▶ ");

    f.render_stateful_widget(list, vert[0], &mut app.repo_mgr_state);

    let hint = if app.repo_form.is_some() {
        " [tab] next field  [enter] confirm  [esc] cancel"
    } else {
        " [j/k] nav  [a]dd  [e]dit  [d]elete  [esc/q] menu"
    };
    let status_text = if app.status.is_empty() {
        hint.to_string()
    } else {
        format!("  {}  │{}", app.status, hint)
    };
    f.render_widget(
        Paragraph::new(status_text).style(Style::default().fg(Color::DarkGray)),
        vert[1],
    );

    if app.repo_form.is_some() {
        draw_repo_form(f, area, app);
    }
}

fn draw_repo_form(f: &mut Frame, area: Rect, app: &App) {
    let form = app.repo_form.as_ref().unwrap();

    let w: u16 = 60;
    let h: u16 = 7;
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    let popup = Rect { x, y, width: w, height: h };

    f.render_widget(Clear, popup);

    let title = if form.editing.is_some() { " Edit Repo " } else { " Add Repo " };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let name_focused = form.focus == RepoFormField::Name;
    let path_focused = form.focus == RepoFormField::Path;

    let name_style = if name_focused { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };
    let path_style = if path_focused { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };

    let name_cursor = if name_focused { "█" } else { "" };
    let path_cursor = if path_focused { "█" } else { "" };

    let lines = vec![
        Line::from(vec![
            Span::styled("Name: ", name_style),
            Span::styled(&form.name, Style::default().fg(Color::White)),
            Span::styled(name_cursor, Style::default().fg(Color::Yellow)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Path: ", path_style),
            Span::styled(&form.path, Style::default().fg(Color::White)),
            Span::styled(path_cursor, Style::default().fg(Color::Yellow)),
        ]),
    ];

    f.render_widget(Paragraph::new(lines), inner);
}

pub fn run(store: Store, registry: Registry, registry_path: PathBuf) -> Result<()> {
    let mut app = App::new(store, registry, registry_path);

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = (|| -> Result<()> {
        loop {
            if let Some(until) = app.notification_until {
                if Instant::now() >= until {
                    app.clear_status();
                }
            }
            if app.needs_clear {
                terminal.clear()?;
                app.needs_clear = false;
            }
            terminal.draw(|f| draw(f, &mut app))?;
            if event::poll(Duration::from_millis(100))? {
                if let Event::Key(key) = event::read()? {
                    app.handle_key(key)?;
                }
            }
            if app.quit { break; }
        }
        Ok(())
    })();

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}
