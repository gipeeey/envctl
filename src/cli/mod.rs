use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "envctl", about = "Manage .env across local repos")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Generate age keypair and initialize config dir
    Init,

    /// Manage rumah sakit (RS) entries
    Rs {
        #[command(subcommand)]
        cmd: RsCommand,
    },

    /// Manage global repositories (shared path, per-RS env)
    Repo {
        #[command(subcommand)]
        cmd: RepoCommand,
    },

    /// Register a repo under an RS
    Register {
        path: String,
        #[arg(long)]
        rs: String,
        #[arg(long)]
        name: Option<String>,
    },

    /// Scan a directory for git repos and register under an RS
    Scan {
        dir: String,
        #[arg(long)]
        rs: String,
    },

    /// List RS, repos, or vars
    List {
        #[command(subcommand)]
        target: ListTarget,
    },

    /// Set a var: KEY=VALUE
    Set {
        rs: String,
        repo: String,
        pair: String,
    },

    /// Get a var value
    Get {
        rs: String,
        repo: String,
        key: String,
        #[arg(long)]
        reveal: bool,
    },

    /// Delete a var
    Delete {
        rs: String,
        repo: String,
        key: String,
    },

    /// Import a .env file into the store
    Import {
        rs: String,
        repo: String,
        env_file: String,
    },

    /// Open $EDITOR to edit all vars for a repo
    Edit {
        rs: String,
        repo: String,
    },

    /// Write .env to repo path(s)
    Apply {
        rs: Option<String>,
        repo: Option<String>,
        #[arg(long)]
        all: bool,
    },

    /// Show diff between store and disk .env
    Diff {
        rs: String,
        repo: String,
    },

    /// Export registry (RS + repo structure) for sharing
    Export {
        #[command(subcommand)]
        target: ExportTarget,
    },

    /// Merge a shared registry file into local registry
    MergeRegistry {
        file: String,
        /// Overwrite existing RS/repo entries with values from file
        #[arg(long)]
        overwrite: bool,
    },

    /// Import a bundle file (registry + all vars)
    ImportBundle {
        file: String,
        /// Overwrite existing entries
        #[arg(long)]
        overwrite: bool,
    },

    /// Dump vars as plaintext .env to stdout (for sharing)
    Dump {
        rs: String,
        repo: String,
    },
}

#[derive(Subcommand)]
pub enum RepoCommand {
    /// Add a global repo: NAME --path PATH
    Add {
        name: String,
        #[arg(long)]
        path: String,
    },
    /// List all global repos
    List,
    /// Remove a global repo
    Remove { name: String },
    /// Update path for a global repo
    Update {
        name: String,
        #[arg(long)]
        path: String,
    },
}

#[derive(Subcommand)]
pub enum RsCommand {
    /// Add an RS: NAME --ip IP
    Add {
        name: String,
        #[arg(long)]
        ip: String,
    },
    /// List all RS entries
    List,
    /// Remove an RS entry
    Remove { name: String },
    /// Search RS by name or IP
    Search { query: String },
}

#[derive(Subcommand)]
pub enum ExportTarget {
    /// Export RS + repo structure only (no secrets)
    Registry {
        /// Write to file instead of stdout
        #[arg(long)]
        output: Option<String>,
    },
    /// Export everything: registry + all vars plaintext (for sharing)
    Bundle {
        /// Write to file instead of stdout
        #[arg(long)]
        output: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum ListTarget {
    /// List all RS entries
    Rs,
    /// List repos (all RS, or filter by --rs)
    Repos {
        #[arg(long)]
        rs: Option<String>,
    },
    /// List vars for a repo
    Vars {
        rs: String,
        repo: String,
        #[arg(long)]
        reveal: bool,
    },
}
