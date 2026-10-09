//! What the host Grant of an Agent on a machine allows its Coding
//! Sessions (ADR-0033).
//!
//! The Grant is read live, so a change applies to the next start. An
//! Agent that holds no live host Grant on the machine gets `person` and
//! no Unattended Mode.

use pagis_core::{
    AgentId, Grant, GrantStore, HostId, SessionApprovalMode, StoreError, WorkspaceId,
};

/// What the live host Grant of an Agent on a machine allows its Coding
/// Sessions there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionAllowance {
    /// The widest Session Approval Mode. A session starts only in a mode
    /// that it permits.
    pub widest_mode: SessionApprovalMode,
    /// Whether a harness may work in an Unattended Mode. A harness that
    /// never asks starts only where it may.
    pub unattended_modes: bool,
}

/// The allowance of the Agent on the machine, from one read of the live
/// host Grant.
pub async fn session_allowance(
    grants: &dyn GrantStore,
    workspace_id: &WorkspaceId,
    agent_id: &AgentId,
    host_id: &HostId,
) -> Result<SessionAllowance, StoreError> {
    let grant = grants
        .live_for_resource(workspace_id, agent_id, Grant::HOST_KIND, host_id.as_str())
        .await?;
    Ok(SessionAllowance {
        widest_mode: grant
            .as_ref()
            .map_or(SessionApprovalMode::Person, Grant::session_approval_mode),
        unattended_modes: grant.as_ref().is_some_and(Grant::unattended_modes),
    })
}

#[cfg(test)]
mod tests {
    use pagis_testkit::{MemoryGrantStore, fixture};

    use super::*;

    struct World {
        grants: MemoryGrantStore,
        workspace_id: WorkspaceId,
        agent_id: AgentId,
        host_id: HostId,
    }

    const NOTHING: SessionAllowance = SessionAllowance {
        widest_mode: SessionApprovalMode::Person,
        unattended_modes: false,
    };

    impl World {
        fn new() -> Self {
            World {
                grants: MemoryGrantStore::default(),
                workspace_id: WorkspaceId::from("w".to_string()),
                agent_id: AgentId::from("a".to_string()),
                host_id: HostId::from("laptop".to_string()),
            }
        }

        /// A live host Grant of the Agent on one machine, in one mode,
        /// that allows Unattended Modes.
        async fn grant(&self, host_id: &HostId, mode: SessionApprovalMode) -> Grant {
            let mut grant = fixture::host_grant(&self.workspace_id, &self.agent_id, host_id, &[]);
            grant.scope = grant.with_session_approval_mode(mode);
            grant.scope = grant.with_unattended_modes(true);
            self.grants.create(&grant).await.expect("write the grant");
            grant
        }

        async fn allowance(&self) -> SessionAllowance {
            session_allowance(
                &self.grants,
                &self.workspace_id,
                &self.agent_id,
                &self.host_id,
            )
            .await
            .expect("read the allowance")
        }
    }

    #[tokio::test]
    async fn the_live_host_grant_of_the_machine_gives_the_allowance() {
        let world = World::new();
        world
            .grant(&world.host_id, SessionApprovalMode::Agent)
            .await;

        assert_eq!(
            world.allowance().await,
            SessionAllowance {
                widest_mode: SessionApprovalMode::Agent,
                unattended_modes: true,
            }
        );
    }

    #[tokio::test]
    async fn no_host_grant_gives_person_and_no_unattended_mode() {
        let world = World::new();

        assert_eq!(world.allowance().await, NOTHING);
    }

    #[tokio::test]
    async fn a_revoked_host_grant_gives_person_and_no_unattended_mode() {
        let world = World::new();
        let grant = world
            .grant(&world.host_id, SessionApprovalMode::Agent)
            .await;
        assert!(
            world
                .grants
                .revoke(&world.workspace_id, &grant.id, 1)
                .await
                .expect("revoke the grant")
        );

        assert_eq!(world.allowance().await, NOTHING);
    }

    #[tokio::test]
    async fn the_grant_of_another_machine_does_not_count() {
        let world = World::new();
        world
            .grant(
                &HostId::from("desktop".to_string()),
                SessionApprovalMode::Agent,
            )
            .await;

        assert_eq!(world.allowance().await, NOTHING);
    }
}
