use super::*;

#[async_trait::async_trait]
impl AgentStore for FakeAgentStore {
    async fn create(&self, agent: &Agent) -> Result<(), StoreError> {
        self.0
            .lock()
            .unwrap()
            .agents
            .insert(&agent.workspace_id, agent.id.clone(), agent.clone())
    }
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentId,
    ) -> Result<Option<Agent>, StoreError> {
        Ok(self.0.lock().unwrap().agents.get(workspace_id, id).cloned())
    }
    async fn list_by_workspace(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Vec<Agent>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .agents
            .rows(workspace_id)
            .cloned()
            .collect())
    }
    async fn update(&self, agent: &Agent) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data.agents.get_mut(&agent.workspace_id, &agent.id) else {
            return Ok(());
        };
        *row = agent.clone();
        Ok(())
    }
    async fn update_avatar(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentId,
        avatar: &crate::AvatarAppearance,
        updated_at: i64,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data.agents.get_mut(workspace_id, id) else {
            return Ok(false);
        };
        row.avatar = avatar.clone();
        row.updated_at = updated_at;
        Ok(true)
    }
}

#[async_trait::async_trait]
impl ModelAliasStore for FakeModelAliasStore {
    async fn create(&self, model_alias: &ModelAlias) -> Result<(), StoreError> {
        self.0.lock().unwrap().model_aliases.insert(
            &model_alias.workspace_id,
            model_alias.alias.clone(),
            model_alias.clone(),
        )
    }
    async fn get_by_alias(
        &self,
        workspace_id: &WorkspaceId,
        alias: &str,
    ) -> Result<Option<ModelAlias>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .model_aliases
            .get(workspace_id, &alias.to_string())
            .cloned())
    }
    async fn list(&self, workspace_id: &WorkspaceId) -> Result<Vec<ModelAlias>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .model_aliases
            .rows(workspace_id)
            .cloned()
            .collect())
    }
    async fn update_candidates(
        &self,
        workspace_id: &WorkspaceId,
        alias: &str,
        candidates: &[String],
        updated_at: UnixMillis,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data.model_aliases.get_mut(workspace_id, &alias.to_string()) else {
            return Ok(false);
        };
        row.candidates = candidates.to_vec();
        row.updated_at = updated_at;
        Ok(true)
    }
    async fn delete(&self, workspace_id: &WorkspaceId, alias: &str) -> Result<bool, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .model_aliases
            .remove(workspace_id, &alias.to_string())
            .is_some())
    }
}

#[async_trait::async_trait]
impl ConnectionStore for FakeConnectionStore {
    async fn create(&self, connection: &Connection) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        if data
            .connections
            .rows(&connection.workspace_id)
            .any(|row| row.alias == connection.alias)
        {
            return Err(conflict("Connection alias exists"));
        }
        data.connections.insert(
            &connection.workspace_id,
            connection.id.clone(),
            connection.clone(),
        )
    }
    async fn set_status(
        &self,
        workspace_id: &WorkspaceId,
        id: &crate::ConnectionId,
        status: &str,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data.connections.get_mut(workspace_id, id) else {
            return Ok(false);
        };
        row.status = status.to_string();
        Ok(true)
    }
    async fn set_authorization(
        &self,
        workspace_id: &WorkspaceId,
        id: &crate::ConnectionId,
        capabilities: &[String],
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data.connections.get_mut(workspace_id, id) else {
            return Ok(false);
        };
        row.authorized_capabilities = capabilities.to_vec();
        Ok(true)
    }
    async fn set_config(
        &self,
        workspace_id: &WorkspaceId,
        id: &crate::ConnectionId,
        config: &serde_json::Value,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data.connections.get_mut(workspace_id, id) else {
            return Ok(false);
        };
        row.config = config.clone();
        Ok(true)
    }
    async fn list(&self, workspace_id: &WorkspaceId) -> Result<Vec<Connection>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .connections
            .rows(workspace_id)
            .cloned()
            .collect())
    }
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &crate::ConnectionId,
    ) -> Result<Option<Connection>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .connections
            .get(workspace_id, id)
            .cloned())
    }
    async fn delete_and_revoke(
        &self,
        workspace_id: &WorkspaceId,
        id: &crate::ConnectionId,
        revoked_at: UnixMillis,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let deleted = data.connections.remove(workspace_id, id).is_some();
        if deleted {
            revoke_resource(
                &mut data,
                workspace_id,
                "connection",
                id.as_str(),
                revoked_at,
            );
            data.refresh_tokens.remove(workspace_id, id);
        }
        Ok(deleted)
    }
    async fn set_refresh_token(
        &self,
        workspace_id: &WorkspaceId,
        id: &crate::ConnectionId,
        token: Option<&crate::SealedSecret>,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        if data.connections.get(workspace_id, id).is_none() {
            return Ok(false);
        }
        if let Some(token) = token {
            data.refresh_tokens
                .put(workspace_id, id.clone(), token.clone());
        } else {
            data.refresh_tokens.remove(workspace_id, id);
        }
        Ok(true)
    }
    async fn refresh_token(
        &self,
        workspace_id: &WorkspaceId,
        id: &crate::ConnectionId,
    ) -> Result<Option<crate::SealedSecret>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .refresh_tokens
            .get(workspace_id, id)
            .cloned())
    }
}

#[async_trait::async_trait]
impl CredentialStore for FakeCredentialStore {
    async fn create(&self, credential: &Credential) -> Result<(), StoreError> {
        self.0.lock().unwrap().credentials.insert(
            &credential.workspace_id,
            credential.id.clone(),
            credential.clone(),
        )
    }
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &crate::CredentialId,
    ) -> Result<Option<Credential>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .credentials
            .get(workspace_id, id)
            .cloned())
    }
    async fn list(
        &self,
        workspace_id: &WorkspaceId,
        domain: Option<&str>,
    ) -> Result<Vec<Credential>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .credentials
            .rows(workspace_id)
            .filter(|row| domain.is_none_or(|domain| row.domain == domain))
            .cloned()
            .collect())
    }
    async fn delete_and_revoke(
        &self,
        workspace_id: &WorkspaceId,
        id: &crate::CredentialId,
        revoked_at: UnixMillis,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let deleted = data.credentials.remove(workspace_id, id).is_some();
        if deleted {
            revoke_resource(
                &mut data,
                workspace_id,
                "credential",
                id.as_str(),
                revoked_at,
            );
        }
        Ok(deleted)
    }
    async fn release_owner(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
    ) -> Result<u64, StoreError> {
        let mut data = self.0.lock().unwrap();
        let mut count = 0;
        for row in data
            .credentials
            .rows_mut(workspace_id)
            .filter(|row| row.owner_agent_id.as_ref() == Some(agent_id))
        {
            row.owner_agent_id = None;
            count += 1;
        }
        Ok(count)
    }
}

#[async_trait::async_trait]
impl GrantStore for FakeGrantStore {
    async fn create(&self, grant: &Grant) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        if grant.revoked_at.is_none()
            && data.grants.rows(&grant.workspace_id).any(|row| {
                row.revoked_at.is_none()
                    && row.agent_id == grant.agent_id
                    && row.resource_kind == grant.resource_kind
                    && row.resource_id == grant.resource_id
                    && (grant.resource_kind != Grant::WORKSPACE_MAILBOX_KIND
                        || (row.scope["default"] == true) == (grant.scope["default"] == true))
            })
        {
            return Err(conflict("live Grant exists"));
        }
        data.grants
            .insert(&grant.workspace_id, grant.id.clone(), grant.clone())
    }
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &GrantId,
    ) -> Result<Option<Grant>, StoreError> {
        Ok(self.0.lock().unwrap().grants.get(workspace_id, id).cloned())
    }
    async fn list_live(&self, workspace_id: &WorkspaceId) -> Result<Vec<Grant>, StoreError> {
        let data = self.0.lock().unwrap();
        let rows = data
            .grants
            .rows(workspace_id)
            .filter(|row| row.revoked_at.is_none() && true);
        Ok(rows.cloned().collect())
    }
    async fn live_grant(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        resource_kind: &str,
    ) -> Result<Option<Grant>, StoreError> {
        let data = self.0.lock().unwrap();
        let rows = data.grants.rows(workspace_id).filter(|row| {
            row.revoked_at.is_none()
                && &row.agent_id == agent_id
                && row.resource_kind == resource_kind
        });
        Ok(rows.cloned().min_by_key(|row| row.created_at))
    }
    async fn list_live_for_agent(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
    ) -> Result<Vec<Grant>, StoreError> {
        let data = self.0.lock().unwrap();
        let rows = data
            .grants
            .rows(workspace_id)
            .filter(|row| row.revoked_at.is_none() && &row.agent_id == agent_id);
        Ok(rows.cloned().collect())
    }
    async fn live_for_resource(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        resource_kind: &str,
        resource_id: &str,
    ) -> Result<Option<Grant>, StoreError> {
        let data = self.0.lock().unwrap();
        let rows = data.grants.rows(workspace_id).filter(|row| {
            row.revoked_at.is_none()
                && &row.agent_id == agent_id
                && row.resource_kind == resource_kind
                && row.resource_id.as_deref() == Some(resource_id)
        });
        Ok(rows.cloned().min_by_key(|row| row.created_at))
    }
    async fn set_scope(
        &self,
        workspace_id: &WorkspaceId,
        id: &GrantId,
        scope: &serde_json::Value,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data
            .grants
            .get_mut(workspace_id, id)
            .filter(|row| row.revoked_at.is_none())
        else {
            return Ok(false);
        };
        row.scope = scope.clone();
        row.revision += 1;
        Ok(true)
    }
    async fn revoke(
        &self,
        workspace_id: &WorkspaceId,
        id: &GrantId,
        revoked_at: UnixMillis,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data
            .grants
            .get_mut(workspace_id, id)
            .filter(|row| row.revoked_at.is_none())
        else {
            return Ok(false);
        };
        row.revoked_at = Some(revoked_at);
        Ok(true)
    }
}

#[async_trait::async_trait]
impl CapabilitySnapshotStore for FakeCapabilitySnapshotStore {
    async fn put(&self, snapshot: &CapabilitySnapshotRecord) -> Result<(), StoreError> {
        self.0
            .lock()
            .unwrap()
            .snapshots
            .entry(snapshot.id.clone())
            .or_insert_with(|| snapshot.clone());
        Ok(())
    }
    async fn bind_run(
        &self,
        workspace_id: &WorkspaceId,
        run_id: &RunId,
        snapshot_id: &str,
    ) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        if !data.snapshots.contains_key(snapshot_id) {
            return Err(conflict("snapshot does not exist"));
        }
        if data.snapshot_runs.get(workspace_id, run_id).is_none() {
            data.snapshot_runs
                .put(workspace_id, run_id.clone(), snapshot_id.to_string());
        }
        Ok(())
    }
    async fn for_run(
        &self,
        workspace_id: &WorkspaceId,
        run_id: &RunId,
    ) -> Result<Option<CapabilitySnapshotRecord>, StoreError> {
        let data = self.0.lock().unwrap();
        Ok(data
            .snapshot_runs
            .get(workspace_id, run_id)
            .and_then(|id| data.snapshots.get(id))
            .cloned())
    }
}

#[async_trait::async_trait]
impl FileStore for FakeFileStore {
    async fn insert(&self, file: &File) -> Result<FileOutcome, StoreError> {
        let mut data = self.0.lock().unwrap();
        if let Some(row) = data
            .files
            .rows(&file.workspace_id)
            .find(|row| row.sha256 == file.sha256)
        {
            return Ok(FileOutcome::Deduplicated(row.clone()));
        }
        data.files
            .insert(&file.workspace_id, file.id.clone(), file.clone())?;
        Ok(FileOutcome::Created(file.clone()))
    }
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &FileId,
    ) -> Result<Option<File>, StoreError> {
        Ok(self.0.lock().unwrap().files.get(workspace_id, id).cloned())
    }
    async fn expired_before(
        &self,
        workspace_id: &WorkspaceId,
        kind: FileKind,
        cutoff: UnixMillis,
        limit: u32,
    ) -> Result<Vec<File>, StoreError> {
        let mut rows: Vec<_> = self
            .0
            .lock()
            .unwrap()
            .files
            .rows(workspace_id)
            .filter(|row| row.kind == kind && row.created_at < cutoff)
            .cloned()
            .collect();
        rows.sort_by_key(|row| (row.created_at, row.id.clone()));
        rows.truncate(limit as usize);
        Ok(rows)
    }
    async fn delete(&self, workspace_id: &WorkspaceId, id: &FileId) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        super::audit::refuse_delete(&data, workspace_id)?;
        Ok(data.files.remove(workspace_id, id).is_some())
    }
}

#[async_trait::async_trait]
impl RetentionPolicyStore for FakeRetentionPolicyStore {
    async fn list(&self, workspace_id: &WorkspaceId) -> Result<Vec<RetentionPolicy>, StoreError> {
        let data = self.0.lock().unwrap();
        Ok(FileKind::ALL
            .into_iter()
            .map(|kind| RetentionPolicy {
                kind,
                retain_days: data
                    .retention
                    .get(workspace_id, &kind.as_str().to_string())
                    .copied()
                    .flatten(),
            })
            .collect())
    }
    async fn set(
        &self,
        workspace_id: &WorkspaceId,
        kind: FileKind,
        retain_days: Option<i64>,
        now: UnixMillis,
    ) -> Result<(), StoreError> {
        let _ = now;
        self.0
            .lock()
            .unwrap()
            .retention
            .put(workspace_id, kind.as_str().to_string(), retain_days);
        Ok(())
    }
}

#[async_trait::async_trait]
impl SoftwareStore for FakeSoftwareStore {
    async fn create_package(&self, package: &SoftwarePackage) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        if data
            .software
            .rows(&package.workspace_id)
            .any(|row| row.name == package.name)
        {
            return Err(conflict("Software Package name exists"));
        }
        data.software
            .insert(&package.workspace_id, package.id.clone(), package.clone())
    }
    async fn get_by_name(
        &self,
        workspace_id: &WorkspaceId,
        name: &str,
    ) -> Result<Option<SoftwarePackage>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .software
            .rows(workspace_id)
            .find(|row| row.name == name)
            .cloned())
    }
    async fn list_packages(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Vec<SoftwarePackage>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .software
            .rows(workspace_id)
            .cloned()
            .collect())
    }
    async fn add_version(
        &self,
        package: &SoftwarePackage,
        version: &SoftwareVersion,
    ) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        if version.package_id != package.id
            || data
                .software
                .get(&package.workspace_id, &package.id)
                .is_none()
        {
            return Err(conflict("Software Package does not exist"));
        }
        data.software_versions.insert(
            &package.workspace_id,
            (package.id.clone(), version.version.clone()),
            version.clone(),
        )?;
        let row = data
            .software
            .get_mut(&package.workspace_id, &package.id)
            .unwrap();
        row.latest_version = version.version.clone();
        row.updated_at = version.published_at;
        Ok(())
    }
    async fn list_versions(
        &self,
        workspace_id: &WorkspaceId,
        package_id: &SoftwarePackageId,
    ) -> Result<Vec<SoftwareVersion>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .software_versions
            .rows(workspace_id)
            .filter(|row| &row.package_id == package_id)
            .cloned()
            .collect())
    }
}

#[async_trait::async_trait]
impl ContributionStore for FakeContributionStore {
    async fn create(&self, contribution: &Contribution) -> Result<(), StoreError> {
        self.0.lock().unwrap().contributions.insert(
            &contribution.workspace_id,
            contribution.id.clone(),
            contribution.clone(),
        )
    }
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &ContributionId,
    ) -> Result<Option<Contribution>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .contributions
            .get(workspace_id, id)
            .cloned())
    }
    async fn list_by_fork(
        &self,
        workspace_id: &WorkspaceId,
        fork_package_id: &SoftwarePackageId,
    ) -> Result<Vec<Contribution>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .contributions
            .rows(workspace_id)
            .filter(|row| &row.fork_package_id == fork_package_id)
            .cloned()
            .collect())
    }
    async fn list_by_package(
        &self,
        workspace_id: &WorkspaceId,
        package_id: &SoftwarePackageId,
    ) -> Result<Vec<Contribution>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .contributions
            .rows(workspace_id)
            .filter(|row| &row.package_id == package_id)
            .cloned()
            .collect())
    }
    async fn close(
        &self,
        workspace_id: &WorkspaceId,
        id: &ContributionId,
        status: ContributionStatus,
        reason: &str,
        closed_at: UnixMillis,
    ) -> Result<(), StoreError> {
        if let Some(row) = self
            .0
            .lock()
            .unwrap()
            .contributions
            .get_mut(workspace_id, id)
        {
            row.status = status;
            row.outcome_reason = Some(reason.to_string());
            row.closed_at = Some(closed_at);
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl PluginStore for FakePluginStore {
    async fn create(&self, plugin: &Plugin, bindings: &[PluginBinding]) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        if data
            .plugins
            .rows(&plugin.workspace_id)
            .any(|row| row.name == plugin.name)
            || bindings
                .iter()
                .any(|binding| binding.plugin_id != plugin.id)
        {
            return Err(conflict("Plugin name or Binding conflicts"));
        }
        data.plugins
            .insert(&plugin.workspace_id, plugin.id.clone(), plugin.clone())?;
        for binding in bindings {
            data.plugin_bindings.put(
                &plugin.workspace_id,
                (plugin.id.clone(), binding.field.clone()),
                binding.clone(),
            );
        }
        Ok(())
    }
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &PluginId,
    ) -> Result<Option<Plugin>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .plugins
            .get(workspace_id, id)
            .cloned())
    }
    async fn get_by_name(
        &self,
        workspace_id: &WorkspaceId,
        name: &str,
    ) -> Result<Option<Plugin>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .plugins
            .rows(workspace_id)
            .find(|row| row.name == name)
            .cloned())
    }
    async fn list(&self, workspace_id: &WorkspaceId) -> Result<Vec<Plugin>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .plugins
            .rows(workspace_id)
            .cloned()
            .collect())
    }
    async fn set_state(
        &self,
        workspace_id: &WorkspaceId,
        id: &PluginId,
        state: PluginState,
        updated_at: UnixMillis,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data.plugins.get_mut(workspace_id, id) else {
            return Ok(false);
        };
        row.state = state;
        row.updated_at = updated_at;
        Ok(true)
    }
    async fn set_installed(
        &self,
        workspace_id: &WorkspaceId,
        id: &PluginId,
        installed_commit: &str,
        manifest_version: &str,
        state: PluginState,
        updated_at: UnixMillis,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data.plugins.get_mut(workspace_id, id) else {
            return Ok(false);
        };
        row.state = state;
        row.installed_commit = installed_commit.to_string();
        row.manifest_version = manifest_version.to_string();
        row.updated_at = updated_at;
        Ok(true)
    }
    async fn put_binding(
        &self,
        workspace_id: &WorkspaceId,
        binding: &PluginBinding,
    ) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        if data.plugins.get(workspace_id, &binding.plugin_id).is_none() {
            return Err(conflict("Plugin does not exist"));
        }
        data.plugin_bindings.put(
            workspace_id,
            (binding.plugin_id.clone(), binding.field.clone()),
            binding.clone(),
        );
        Ok(())
    }
    async fn list_bindings(
        &self,
        workspace_id: &WorkspaceId,
        id: &PluginId,
    ) -> Result<Vec<PluginBinding>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .plugin_bindings
            .rows(workspace_id)
            .filter(|row| &row.plugin_id == id)
            .cloned()
            .collect())
    }
    async fn delete_binding(
        &self,
        workspace_id: &WorkspaceId,
        id: &PluginId,
        field: &str,
    ) -> Result<(), StoreError> {
        self.0
            .lock()
            .unwrap()
            .plugin_bindings
            .remove(workspace_id, &(id.clone(), field.to_string()));
        Ok(())
    }
    async fn delete_and_revoke(
        &self,
        workspace_id: &WorkspaceId,
        id: &PluginId,
        revoked_at: UnixMillis,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let deleted = data.plugins.remove(workspace_id, id).is_some();
        if deleted {
            for grant in data.grants.0.values_mut() {
                if grant.resource_kind == Grant::PLUGIN_KIND
                    && grant.resource_id.as_deref() == Some(id.as_str())
                    && grant.revoked_at.is_none()
                {
                    grant.revoked_at = Some(revoked_at);
                }
            }
            data.plugin_bindings
                .0
                .retain(|(w, (p, _)), _| w != workspace_id || p != id);
            data.plugin_tools.retain(|(p, _), _| p != id);
        }
        Ok(deleted)
    }
}

#[async_trait::async_trait]
impl PluginToolStore for FakePluginToolStore {
    async fn put(&self, tools: &PluginTools) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        if !data.plugins.0.values().any(|row| row.id == tools.plugin_id) {
            return Err(conflict("Plugin does not exist"));
        }
        data.plugin_tools.insert(
            (tools.plugin_id.clone(), tools.version.clone()),
            tools.clone(),
        );
        Ok(())
    }
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        plugin_id: &PluginId,
        version: &str,
    ) -> Result<Option<PluginTools>, StoreError> {
        let data = self.0.lock().unwrap();
        Ok(data
            .plugins
            .get(workspace_id, plugin_id)
            .and_then(|_| {
                data.plugin_tools
                    .get(&(plugin_id.clone(), version.to_string()))
            })
            .cloned())
    }
    async fn set_tools_changed(
        &self,
        workspace_id: &WorkspaceId,
        plugin_id: &PluginId,
        version: &str,
        changed: bool,
    ) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        if data.plugins.get(workspace_id, plugin_id).is_some()
            && let Some(row) = data
                .plugin_tools
                .get_mut(&(plugin_id.clone(), version.to_string()))
        {
            row.tools_changed = changed;
        }
        Ok(())
    }
    async fn delete(
        &self,
        workspace_id: &WorkspaceId,
        plugin_id: &PluginId,
    ) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        if data.plugins.get(workspace_id, plugin_id).is_some() {
            data.plugin_tools.retain(|(p, _), _| p != plugin_id);
        }
        Ok(())
    }
}

fn revoke_resource(data: &mut Data, workspace: &WorkspaceId, kind: &str, id: &str, at: i64) {
    for grant in data.grants.rows_mut(workspace).filter(|g| {
        g.resource_kind == kind && g.resource_id.as_deref() == Some(id) && g.revoked_at.is_none()
    }) {
        grant.revoked_at = Some(at);
    }
}
