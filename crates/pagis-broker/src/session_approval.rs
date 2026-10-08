//! The widest Session Approval Mode of an Agent on a machine (ADR-0033).
//!
//! The host Grant of the Agent on that machine holds it. The Grant is
//! read live, so a change of mode applies to the next start. An Agent
//! that holds no live host Grant on the machine gets `person`.

use pagis_core::{
    AgentId, Grant, GrantStore, HostId, SessionApprovalMode, StoreError, WorkspaceId,
};

/// The widest Session Approval Mode that the Agent may use on the
/// machine. A Coding Session starts only in a mode that it permits.
pub async fn widest_session_approval_mode(
    grants: &dyn GrantStore,
    workspace_id: &WorkspaceId,
    agent_id: &AgentId,
    host_id: &HostId,
) -> Result<SessionApprovalMode, StoreError> {
    Ok(grants
        .live_for_resource(workspace_id, agent_id, Grant::HOST_KIND, host_id.as_str())
        .await?
        .map_or(SessionApprovalMode::Person, |grant| {
            grant.session_approval_mode()
        }))
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

    impl World {
        fn new() -> Self {
            World {
                grants: MemoryGrantStore::default(),
                workspace_id: WorkspaceId::from("w".to_string()),
                agent_id: AgentId::from("a".to_string()),
                host_id: HostId::from("laptop".to_string()),
            }
        }

        /// A live host Grant of the Agent on one machine, in one mode.
        async fn grant(&self, host_id: &HostId, mode: SessionApprovalMode) -> Grant {
            let mut grant = fixture::host_grant(&self.workspace_id, &self.agent_id, host_id, &[]);
            grant.scope = grant.with_session_approval_mode(mode);
            self.grants.create(&grant).await.expect("write the grant");
            grant
        }

        async fn widest(&self) -> SessionApprovalMode {
            widest_session_approval_mode(
                &self.grants,
                &self.workspace_id,
                &self.agent_id,
                &self.host_id,
            )
            .await
            .expect("read the widest mode")
        }
    }

    #[tokio::test]
    async fn the_live_host_grant_of_the_machine_gives_the_mode() {
        let world = World::new();
        world
            .grant(&world.host_id, SessionApprovalMode::Agent)
            .await;

        assert_eq!(world.widest().await, SessionApprovalMode::Agent);
    }

    #[tokio::test]
    async fn no_host_grant_gives_person() {
        let world = World::new();

        assert_eq!(world.widest().await, SessionApprovalMode::Person);
    }

    #[tokio::test]
    async fn a_revoked_host_grant_gives_person() {
        let world = World::new();
        let grant = world.grant(&world.host_id, SessionApprovalMode::Auto).await;
        assert!(
            world
                .grants
                .revoke(&world.workspace_id, &grant.id, 1)
                .await
                .expect("revoke the grant")
        );

        assert_eq!(world.widest().await, SessionApprovalMode::Person);
    }

    #[tokio::test]
    async fn the_grant_of_another_machine_does_not_count() {
        let world = World::new();
        world
            .grant(
                &HostId::from("desktop".to_string()),
                SessionApprovalMode::Auto,
            )
            .await;

        assert_eq!(world.widest().await, SessionApprovalMode::Person);
    }
}
