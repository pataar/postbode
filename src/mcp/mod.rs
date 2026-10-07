//! `postbode mcp`: the agent surface over MCP on stdio, limited by scopes set in the host's config.
mod backend;
pub mod install;
#[cfg(test)]
mod junk_sweep;
#[cfg(test)]
mod tests;
mod tools;

use std::collections::BTreeSet;

use anyhow::{Result, bail};
use rmcp::ServiceExt;

pub use backend::Backend;
pub use tools::Server;

use crate::config::Config;
use crate::paths::Paths;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    MailModify,
    Read,
    ReadBodies,
    RulesPropose,
    RulesWrite,
}

impl Scope {
    pub const ALL: [Scope; 5] = [
        Scope::Read,
        Scope::ReadBodies,
        Scope::RulesPropose,
        Scope::RulesWrite,
        Scope::MailModify,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Scope::MailModify => "mail:modify",
            Scope::Read => "read",
            Scope::ReadBodies => "read:bodies",
            Scope::RulesPropose => "rules:propose",
            Scope::RulesWrite => "rules:write",
        }
    }
}

/// A comma-separated scope list; an unknown scope, or none at all, names the valid ones.
pub fn parse_scopes(list: &str) -> Result<BTreeSet<Scope>> {
    let valid = || Scope::ALL.map(Scope::as_str).join(", ");
    let mut scopes = BTreeSet::new();
    for word in list.split(',').map(str::trim).filter(|w| !w.is_empty()) {
        match Scope::ALL.into_iter().find(|s| s.as_str() == word) {
            Some(scope) => scopes.insert(scope),
            None => bail!("unknown scope '{word}'; valid scopes: {}", valid()),
        };
    }
    if scopes.is_empty() {
        bail!("no scopes given; valid scopes: {}", valid());
    }
    Ok(scopes)
}

/// Serves on stdin and stdout until the host closes the pipe.
pub fn run(config: &Config, paths: &Paths, scopes: &str, accounts: &[String]) -> Result<()> {
    let server = Server::new(
        Backend::new(config, paths, accounts)?,
        parse_scopes(scopes)?,
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let running = server.serve(rmcp::transport::stdio()).await?;
        running.waiting().await?;
        Ok(())
    })
}
