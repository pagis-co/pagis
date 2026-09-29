//! Contribute a change from a Fork back to its origin package
//! (ADR-0016).
//!
//! A Contribution is a record and a message, not a review surface. The
//! daemon computes the patch between the base tree and the Fork tree,
//! stores it, and posts it as a message from the forker into the two
//! agents' direct channel. The author reads it like any agent message,
//! merges it in its own working copy, publishes a new Version, and
//! closes the record. The close posts the outcome to the forker in the
//! same channel.

use std::sync::Arc;

use async_trait::async_trait;
use pagis_core::{
    AgentId, AgentStore, Contribution, ContributionId, ContributionStatus, ContributionStore,
    EventBus, NewEvent, RunId, SoftwarePackage, SoftwareStore, WorkspaceId, now_ms,
};

use crate::git::{SoftwareGitStore, VersionRef};
use crate::note::{SoftwareNotes, close_note, contribute_note};

/// The largest patch a message carries inline. A larger one is
/// announced by its file list, and the author asks for the files.
pub const MAX_INLINE_PATCH_BYTES: usize = 32 * 1024;

/// How the daemon posts a message as one Agent to another. The daemon
/// fills it with the same path `send_message` takes, so a Contribution
/// wakes the author exactly as an agent message does (ADR-0003).
#[async_trait]
pub trait AgentMessenger: Send + Sync {
    async fn post(
        &self,
        workspace_id: &WorkspaceId,
        from: &AgentId,
        to: &AgentId,
        run_id: &RunId,
        text: &str,
    ) -> Result<(), String>;
}

pub struct ContributionsDeps {
    pub packages: Arc<dyn SoftwareStore>,
    pub contributions: Arc<dyn ContributionStore>,
    pub agents: Arc<dyn AgentStore>,
    pub git: Arc<SoftwareGitStore>,
    pub messenger: Arc<dyn AgentMessenger>,
    pub bus: Arc<dyn EventBus>,
    /// Where an opened and a closed Contribution are written down.
    pub notes: Arc<dyn SoftwareNotes>,
}

pub struct Contributions {
    deps: ContributionsDeps,
}

impl Contributions {
    pub fn new(deps: ContributionsDeps) -> Self {
        Self { deps }
    }

    /// Open one Contribution from the caller's Fork of `package`.
    /// Every precondition is a rule: the caller holds a
    /// published Fork of the package, the Fork has no open
    /// Contribution, and the patch is not empty.
    pub async fn contribute(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        run_id: &RunId,
        package: &str,
        summary: &str,
    ) -> Result<String, String> {
        let origin = self.package_named(workspace_id, package).await?;
        if &origin.author_agent_id == agent_id {
            return Err(format!(
                "you are the author of {package}; publish a new version instead of contributing \
                 to yourself"
            ));
        }
        let fork = self.fork_of(workspace_id, agent_id, &origin).await?;
        let open = self
            .deps
            .contributions
            .list_by_fork(workspace_id, &fork.id)
            .await
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|held| held.status == ContributionStatus::Open);
        if let Some(open) = open {
            return Err(format!(
                "contribution {} from {} is still open; wait for {} to close it",
                open.id, fork.name, package
            ));
        }

        let base = self.base_version(workspace_id, &fork, &origin).await?;
        let patch = self
            .deps
            .git
            .patch(
                workspace_id,
                VersionRef {
                    package: &base.package,
                    version: &base.version,
                },
                VersionRef {
                    package: &fork.name,
                    version: &fork.latest_version,
                },
            )
            .await
            .map_err(|error| error.to_string())?;
        if patch.is_empty() {
            return Err(format!(
                "{} {} is the same as {} {}; there is nothing to contribute",
                fork.name, fork.latest_version, base.package, base.version
            ));
        }

        let contribution = Contribution {
            id: ContributionId::generate(),
            workspace_id: workspace_id.clone(),
            package_id: origin.id.clone(),
            base_version: base.version.clone(),
            latest_at_open: origin.latest_version.clone(),
            fork_package_id: fork.id.clone(),
            fork_version: fork.latest_version.clone(),
            patch: patch.text.clone(),
            summary: summary.to_string(),
            status: ContributionStatus::Open,
            outcome_reason: None,
            created_at: now_ms(),
            closed_at: None,
            run_id: run_id.clone(),
        };
        self.deps
            .contributions
            .create(&contribution)
            .await
            .map_err(|error| error.to_string())?;

        self.deps
            .messenger
            .post(
                workspace_id,
                agent_id,
                &origin.author_agent_id,
                run_id,
                &contribution_message(&contribution, &origin, &fork, &base, &patch.files),
            )
            .await?;
        self.announce(workspace_id, &contribution, &origin.name)
            .await;
        self.note(
            workspace_id,
            agent_id,
            run_id,
            &contribute_note(contribution.id.as_str(), &origin.name, summary),
        )
        .await;
        Ok(format!(
            "Contribution {} is open: {} {} against {} {}. {} has the patch as a message.",
            contribution.id,
            fork.name,
            fork.latest_version,
            base.package,
            base.version,
            self.agent_name(workspace_id, &origin.author_agent_id).await
        ))
    }

    /// The record and its patch, or the diff of one file of it.
    /// The forker and the author read it; nobody else does.
    pub async fn view(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        id: &str,
        path: Option<&str>,
    ) -> Result<String, String> {
        let (contribution, origin, fork) = self.readable(workspace_id, agent_id, id).await?;
        let mut text = format!(
            "Contribution {}\nstatus: {}\nto: {} (base {}, latest at open {})\nfrom: {} \
             {}\nsummary: {}\n",
            contribution.id,
            contribution.status.as_str(),
            origin.name,
            contribution.base_version,
            contribution.latest_at_open,
            fork.name,
            contribution.fork_version,
            contribution.summary,
        );
        if let Some(reason) = &contribution.outcome_reason {
            text.push_str(&format!("outcome: {reason}\n"));
        }
        match path {
            Some(path) => {
                let patch = crate::git::Patch {
                    text: contribution.patch.clone(),
                    files: Vec::new(),
                };
                let file = patch
                    .file(path)
                    .ok_or_else(|| format!("the patch does not change {path}"))?;
                text.push_str(&format!("\n{file}"));
            }
            None => text.push_str(&format!("\n{}", contribution.patch)),
        }
        Ok(text)
    }

    /// Close one Contribution as merged or declined. Only the
    /// author of the origin package closes it, and `merged` needs a
    /// Version published after the record opened: the daemon never
    /// applies a patch, so a merge is proved by a publish.
    pub async fn close(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        run_id: &RunId,
        id: &str,
        status: &str,
        reason: &str,
    ) -> Result<String, String> {
        let status = match ContributionStatus::parse(status) {
            Some(ContributionStatus::Merged) => ContributionStatus::Merged,
            Some(ContributionStatus::Declined) => ContributionStatus::Declined,
            _ => return Err(format!("{status:?} is not merged or declined")),
        };
        let contribution = self.record(workspace_id, id).await?;
        let origin = self
            .package_with_id(workspace_id, &contribution.package_id)
            .await?;
        if &origin.author_agent_id != agent_id {
            return Err(format!(
                "{} is offered to {}, and only its author closes it",
                contribution.id, origin.name
            ));
        }
        if contribution.status != ContributionStatus::Open {
            return Err(format!(
                "{} is already {}",
                contribution.id,
                contribution.status.as_str()
            ));
        }
        let fork = self
            .package_with_id(workspace_id, &contribution.fork_package_id)
            .await?;

        let merged_version = match status {
            ContributionStatus::Merged => {
                let published = self
                    .deps
                    .packages
                    .list_versions(workspace_id, &origin.id)
                    .await
                    .map_err(|error| error.to_string())?
                    .into_iter()
                    .rfind(|version| version.published_at > contribution.created_at);
                match published {
                    Some(version) => Some(version.version),
                    None => {
                        return Err(format!(
                            "{} has published no version since {} opened; apply the patch and \
                             software_publish {} first",
                            origin.name, contribution.id, origin.name
                        ));
                    }
                }
            }
            _ => None,
        };

        let now = now_ms();
        self.deps
            .contributions
            .close(workspace_id, &contribution.id, status, reason, now)
            .await
            .map_err(|error| error.to_string())?;

        let outcome = match &merged_version {
            Some(version) => format!(
                "Contribution {}: merged in {} {}. {reason}",
                contribution.id, origin.name, version
            ),
            None => format!("Contribution {}: declined: {reason}", contribution.id),
        };
        self.deps
            .messenger
            .post(
                workspace_id,
                agent_id,
                &fork.author_agent_id,
                run_id,
                &outcome,
            )
            .await?;
        let closed = Contribution {
            status,
            ..contribution.clone()
        };
        self.announce(workspace_id, &closed, &origin.name).await;
        self.note(
            workspace_id,
            agent_id,
            run_id,
            &close_note(
                contribution.id.as_str(),
                &origin.name,
                merged_version.as_deref(),
                reason,
            ),
        )
        .await;
        Ok(outcome)
    }

    /// Tell the desk that one Contribution changed. An event
    /// that cannot be published is logged: the record already landed.
    async fn announce(
        &self,
        workspace_id: &WorkspaceId,
        contribution: &Contribution,
        package: &str,
    ) {
        let published = self
            .deps
            .bus
            .publish(NewEvent {
                workspace_id: workspace_id.clone(),
                event_type: "contribution.updated".to_string(),
                agent_id: None,
                run_id: Some(contribution.run_id.clone()),
                channel_id: None,
                payload: serde_json::json!({
                    "package": package,
                    "id": contribution.id.as_str(),
                    "status": contribution.status.as_str(),
                }),
            })
            .await;
        if let Err(problem) = published {
            tracing::error!(%problem, "contribution.updated was not published");
        }
    }

    /// Write one line into the acting Agent's software notes.
    /// A note that cannot be written is logged: the action it
    /// describes already landed.
    async fn note(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        run_id: &RunId,
        line: &str,
    ) {
        if let Err(problem) = self
            .deps
            .notes
            .record(workspace_id, agent_id, run_id, line)
            .await
        {
            tracing::error!(problem, line, "the software note was not written");
        }
    }

    /// The record, the origin package and the Fork, when the caller is
    /// one of the two agents the Contribution is between.
    async fn readable(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        id: &str,
    ) -> Result<(Contribution, SoftwarePackage, SoftwarePackage), String> {
        let contribution = self.record(workspace_id, id).await?;
        let origin = self
            .package_with_id(workspace_id, &contribution.package_id)
            .await?;
        let fork = self
            .package_with_id(workspace_id, &contribution.fork_package_id)
            .await?;
        if &origin.author_agent_id != agent_id && &fork.author_agent_id != agent_id {
            return Err(format!("contribution {id} is not yours to read"));
        }
        Ok((contribution, origin, fork))
    }

    async fn record(&self, workspace_id: &WorkspaceId, id: &str) -> Result<Contribution, String> {
        self.deps
            .contributions
            .get(workspace_id, &ContributionId::from(id.to_string()))
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("there is no contribution {id}"))
    }

    async fn package_named(
        &self,
        workspace_id: &WorkspaceId,
        name: &str,
    ) -> Result<SoftwarePackage, String> {
        self.deps
            .packages
            .get_by_name(workspace_id, name)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("there is no package {name}"))
    }

    async fn package_with_id(
        &self,
        workspace_id: &WorkspaceId,
        id: &pagis_core::SoftwarePackageId,
    ) -> Result<SoftwarePackage, String> {
        self.deps
            .packages
            .list_packages(workspace_id)
            .await
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|package| &package.id == id)
            .ok_or_else(|| format!("there is no package {id}"))
    }

    /// The caller's published Fork of `origin`. An unpublished
    /// directory is not a package, so a forker that did not publish is
    /// told to publish first.
    async fn fork_of(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        origin: &SoftwarePackage,
    ) -> Result<SoftwarePackage, String> {
        let forks: Vec<SoftwarePackage> = self
            .deps
            .packages
            .list_packages(workspace_id)
            .await
            .map_err(|error| error.to_string())?
            .into_iter()
            .filter(|package| {
                &package.author_agent_id == agent_id
                    && package.origin_package_id.as_ref() == Some(&origin.id)
            })
            .collect();
        match forks.len() {
            0 => Err(format!(
                "you have no published fork of {}; software_fork it, change it, and \
                 software_publish it first",
                origin.name
            )),
            1 => Ok(forks.into_iter().next().expect("one fork")),
            _ => Err(format!(
                "you hold more than one fork of {}: {}. Keep one fork per package to contribute \
                 from",
                origin.name,
                forks
                    .iter()
                    .map(|fork| fork.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    /// What the patch is against. The first Contribution starts at the
    /// origin Version the Fork was taken from; a later one starts at
    /// the Fork Version the last merged Contribution carried, so it
    /// holds only the change since then.
    async fn base_version(
        &self,
        workspace_id: &WorkspaceId,
        fork: &SoftwarePackage,
        origin: &SoftwarePackage,
    ) -> Result<Base, String> {
        let merged = self
            .deps
            .contributions
            .list_by_fork(workspace_id, &fork.id)
            .await
            .map_err(|error| error.to_string())?
            .into_iter()
            .rfind(|held| held.status == ContributionStatus::Merged);
        match merged {
            Some(merged) => Ok(Base {
                package: fork.name.clone(),
                version: merged.fork_version,
            }),
            None => Ok(Base {
                package: origin.name.clone(),
                version: fork
                    .origin_version
                    .clone()
                    .ok_or_else(|| format!("{} records no origin version", fork.name))?,
            }),
        }
    }

    async fn agent_name(&self, workspace_id: &WorkspaceId, agent_id: &AgentId) -> String {
        match self.deps.agents.get(workspace_id, agent_id).await {
            Ok(Some(agent)) => agent.name,
            _ => agent_id.to_string(),
        }
    }
}

/// The Version one patch is against, and the package it belongs to.
pub struct Base {
    pub package: String,
    pub version: String,
}

/// The message the forker posts to the author (ADR-0016). The
/// patch is inline while it is small; a larger one is announced by the
/// files it changes.
fn contribution_message(
    contribution: &Contribution,
    origin: &SoftwarePackage,
    fork: &SoftwarePackage,
    base: &Base,
    files: &[String],
) -> String {
    let mut text = format!(
        "Contribution `{}` to `{}` from `{}`: {}\n\nBase {} {}",
        contribution.id, origin.name, fork.name, contribution.summary, base.package, base.version,
    );
    if base.package == origin.name && base.version != contribution.latest_at_open {
        text.push_str(&format!(
            ", and {} is now at {}",
            origin.name, contribution.latest_at_open
        ));
    }
    text.push_str(&format!(
        ". The change is {} {}.\n\n",
        fork.name, contribution.fork_version
    ));
    if contribution.patch.len() <= MAX_INLINE_PATCH_BYTES {
        text.push_str(&format!("```diff\n{}```\n", contribution.patch));
    } else {
        text.push_str(&format!(
            "The patch is {} bytes, too large for a message. It changes:\n",
            contribution.patch.len()
        ));
        for file in files {
            text.push_str(&format!("- {file}\n"));
        }
        text.push_str(&format!(
            "\nRead it with contribution_view {{ id: \"{}\", path: \"<file>\" }}.\n",
            contribution.id
        ));
    }
    text
}
