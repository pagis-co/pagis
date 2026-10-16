use super::*;

#[async_trait::async_trait]
impl WorkspaceStore for FakeWorkspaceStore {
    async fn start_legal_hold(&self, entry: &AuditEntry) -> Result<(), StoreError> {
        let _hold = LegalHold::lock(&entry.subject_workspace_id).await;
        super::audit::change_hold(&mut self.0.lock().unwrap(), entry, true)
    }
    async fn end_legal_hold(&self, entry: &AuditEntry) -> Result<(), StoreError> {
        let _hold = LegalHold::lock(&entry.subject_workspace_id).await;
        super::audit::change_hold(&mut self.0.lock().unwrap(), entry, false)
    }

    async fn set_monthly_spend_cap(
        &self,
        id: &WorkspaceId,
        cap_usd: Option<f64>,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data.workspaces.get_mut(id) else {
            return Ok(false);
        };
        row.monthly_spend_cap_usd = cap_usd;
        Ok(true)
    }
    async fn create(&self, workspace: &Workspace) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        if data.workspaces.values().any(|w| {
            w.id == workspace.id || (w.user_id.is_some() && w.user_id == workspace.user_id)
        }) || data.orgs.values().any(|o| o.workspace_id == workspace.id)
        {
            return Err(conflict("Workspace exists"));
        }
        data.workspaces
            .insert(workspace.id.clone(), workspace.clone());
        Ok(())
    }
    async fn get(&self, id: &WorkspaceId) -> Result<Option<Workspace>, StoreError> {
        Ok(self.0.lock().unwrap().workspaces.get(id).cloned())
    }
    async fn for_user(&self, user_id: &UserId) -> Result<Option<Workspace>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .workspaces
            .values()
            .find(|w| w.user_id.as_ref() == Some(user_id))
            .cloned())
    }
    async fn list(&self) -> Result<Vec<Workspace>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .workspaces
            .values()
            .cloned()
            .collect())
    }
    async fn set_onboarded(&self, id: &WorkspaceId, at: UnixMillis) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        if let Some(row) = data.workspaces.get_mut(id) {
            row.onboarded_at.get_or_insert(at);
        }
        Ok(())
    }
    async fn set_timezone(&self, id: &WorkspaceId, timezone: &str) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        if let Some(row) = data.workspaces.get_mut(id) {
            row.timezone = timezone.to_string();
        }
        Ok(())
    }
    async fn set_chief_of_staff(
        &self,
        id: &WorkspaceId,
        agent_id: Option<&AgentId>,
    ) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        if let Some(row) = data.workspaces.get_mut(id) {
            row.chief_of_staff_agent_id = agent_id.cloned();
        }
        Ok(())
    }
    async fn set_report_schedule(
        &self,
        id: &WorkspaceId,
        schedule_id: Option<&ScheduleId>,
    ) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        if let Some(row) = data.workspaces.get_mut(id) {
            row.report_schedule_id = schedule_id.cloned();
        }
        Ok(())
    }
    async fn set_home_exit(
        &self,
        id: &WorkspaceId,
        host_id: Option<&HostId>,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        if host_id.is_some_and(|host| data.hosts.get(id, host).is_none()) {
            return Ok(false);
        }
        let Some(row) = data.workspaces.get_mut(id) else {
            return Ok(false);
        };
        row.home_exit_host_id = host_id.cloned();
        Ok(true)
    }
}

#[async_trait::async_trait]
impl OnboardingStore for FakeOnboardingStore {
    async fn model_verifications(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Vec<OnboardingModelVerification>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .onboarding
            .rows(workspace_id)
            .cloned()
            .collect())
    }
    async fn set_model_verification(
        &self,
        workspace_id: &WorkspaceId,
        verification: &OnboardingModelVerification,
    ) -> Result<(), StoreError> {
        self.0.lock().unwrap().onboarding.put(
            workspace_id,
            verification.provider.clone(),
            verification.clone(),
        );
        Ok(())
    }
}

#[async_trait::async_trait]
impl OrgStore for FakeOrgStore {
    async fn create(&self, org: &Org) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        if data.orgs.contains_key(&org.id) {
            return Err(conflict("Org exists"));
        }
        if data.workspaces.contains_key(&org.workspace_id) {
            return Err(conflict("Org's Workspace belongs to a Person"));
        }
        data.orgs.insert(org.id.clone(), org.clone());
        Ok(())
    }
    async fn list(&self) -> Result<Vec<Org>, StoreError> {
        Ok(self.0.lock().unwrap().orgs.values().cloned().collect())
    }
    async fn person_defaults(&self, id: &OrgId) -> Result<crate::PersonDefaults, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .person_defaults
            .get(id)
            .cloned()
            .unwrap_or_default())
    }
    async fn set_person_defaults(
        &self,
        id: &OrgId,
        defaults: &crate::PersonDefaults,
        entry: &crate::AuditEntry,
    ) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        if !data.orgs.contains_key(id) {
            return Err(StoreError::Conflict("Org is missing".into()));
        }
        if data.audit.contains_key(&entry.id) {
            return Err(conflict("The Audit Entry exists."));
        }
        data.person_defaults.insert(id.clone(), defaults.clone());
        data.audit.insert(entry.id.clone(), entry.clone());
        Ok(())
    }
    async fn set_google_client_id(
        &self,
        id: &OrgId,
        client_id: Option<&str>,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data.orgs.get_mut(id) else {
            return Ok(false);
        };
        row.google_client_id = client_id.map(str::to_string);
        Ok(true)
    }
}

#[async_trait::async_trait]
impl UserStore for FakeUserStore {
    async fn create(&self, user: &User) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        if data.users.contains_key(&user.id)
            || user.email.as_ref().is_some_and(|e| {
                data.users.values().any(|u| {
                    u.email
                        .as_ref()
                        .is_some_and(|held| held.eq_ignore_ascii_case(e))
                })
            })
        {
            return Err(conflict("Person exists"));
        }
        data.users.insert(user.id.clone(), user.clone());
        Ok(())
    }
    async fn get(&self, id: &UserId) -> Result<Option<User>, StoreError> {
        Ok(self.0.lock().unwrap().users.get(id).cloned())
    }
    async fn find_by_email(&self, email: &str) -> Result<Option<User>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .users
            .values()
            .find(|row| {
                row.email
                    .as_ref()
                    .is_some_and(|held| held.eq_ignore_ascii_case(email))
            })
            .cloned())
    }
    async fn list_by_org(&self, org_id: &OrgId) -> Result<Vec<User>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .users
            .values()
            .filter(|row| &row.org_id == org_id)
            .cloned()
            .collect())
    }
    async fn set_name(&self, id: &UserId, name: &str, at: UnixMillis) -> Result<(), StoreError> {
        if let Some(row) = self.0.lock().unwrap().users.get_mut(id) {
            row.name = Some(name.to_string());
            row.updated_at = at;
        }
        Ok(())
    }
    async fn set_email_and_password(
        &self,
        id: &UserId,
        email: &str,
        password_hash: &str,
        at: UnixMillis,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        if data.users.values().any(|u| {
            &u.id != id
                && u.email
                    .as_ref()
                    .is_some_and(|e| e.eq_ignore_ascii_case(email))
        }) {
            return Err(conflict("email exists"));
        }
        let Some(row) = data.users.get_mut(id) else {
            return Ok(false);
        };
        row.email = Some(email.to_string());
        row.password_hash = Some(password_hash.to_string());
        row.updated_at = at;
        Ok(true)
    }
    async fn claim_first_administrator(
        &self,
        id: &UserId,
        email: &str,
        password_hash: &str,
        at: UnixMillis,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        if data
            .users
            .values()
            .any(|u| u.role == UserRole::Administrator && u.password_hash.is_some())
        {
            return Ok(false);
        }
        if data.users.values().any(|u| {
            &u.id != id
                && u.email
                    .as_ref()
                    .is_some_and(|e| e.eq_ignore_ascii_case(email))
        }) {
            return Err(conflict("email exists"));
        }
        let Some(row) = data
            .users
            .get_mut(id)
            .filter(|u| u.role == UserRole::Administrator && u.password_hash.is_none())
        else {
            return Ok(false);
        };
        row.email = Some(email.to_string());
        row.password_hash = Some(password_hash.to_string());
        row.updated_at = at;
        Ok(true)
    }
    async fn set_password_hash(
        &self,
        id: &UserId,
        hash: Option<&str>,
        at: UnixMillis,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data.users.get_mut(id) else {
            return Ok(false);
        };
        row.password_hash = hash.map(str::to_string);
        row.updated_at = at;
        Ok(true)
    }
    async fn set_disabled(
        &self,
        id: &UserId,
        disabled_at: Option<UnixMillis>,
        at: UnixMillis,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data.users.get_mut(id) else {
            return Ok(false);
        };
        row.disabled_at = disabled_at;
        row.updated_at = at;
        Ok(true)
    }
    async fn set_role(
        &self,
        id: &UserId,
        role: UserRole,
        at: UnixMillis,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(person) = data.users.get(id) else {
            return Ok(false);
        };
        if role == UserRole::Member
            && person.role == UserRole::Administrator
            && !person.is_disabled()
            && !data.users.values().any(|other| {
                other.org_id == person.org_id
                    && other.id != person.id
                    && other.role == UserRole::Administrator
                    && !other.is_disabled()
            })
        {
            return Err(conflict(
                "The Org must have at least one enabled Administrator.",
            ));
        }
        let person = data.users.get_mut(id).unwrap();
        person.role = role;
        person.updated_at = at;
        Ok(true)
    }
    async fn record_sign_in(&self, id: &UserId, at: UnixMillis) -> Result<(), StoreError> {
        if let Some(row) = self.0.lock().unwrap().users.get_mut(id) {
            row.last_signed_in_at = Some(at);
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl SessionStore for FakeSessionStore {
    async fn create(&self, session: &Session) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        if data.sessions.contains_key(&session.id)
            || data
                .sessions
                .values()
                .any(|s| s.token_hash == session.token_hash)
        {
            return Err(conflict("Session exists"));
        }
        data.sessions.insert(session.id.clone(), session.clone());
        Ok(())
    }
    async fn find_live(
        &self,
        token_hash: &str,
        now: UnixMillis,
    ) -> Result<Option<Session>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .sessions
            .values()
            .find(|s| s.token_hash == token_hash && s.expires_at > now)
            .cloned())
    }
    async fn find_live_by_id(
        &self,
        id: &SessionId,
        now: UnixMillis,
    ) -> Result<Option<Session>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .sessions
            .get(id)
            .filter(|s| s.expires_at > now)
            .cloned())
    }
    async fn touch(
        &self,
        id: &SessionId,
        at: UnixMillis,
        expires_at: UnixMillis,
    ) -> Result<(), StoreError> {
        if let Some(row) = self.0.lock().unwrap().sessions.get_mut(id) {
            row.last_used_at = at;
            row.expires_at = expires_at;
        }
        Ok(())
    }
    async fn list_live(&self, now: UnixMillis) -> Result<Vec<Session>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .sessions
            .values()
            .filter(|s| s.expires_at > now)
            .cloned()
            .collect())
    }
    async fn list_live_for_user(
        &self,
        user_id: &UserId,
        now: UnixMillis,
    ) -> Result<Vec<Session>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .sessions
            .values()
            .filter(|s| &s.user_id == user_id && s.expires_at > now)
            .cloned()
            .collect())
    }
    async fn delete(&self, id: &SessionId) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let removed = data.sessions.remove(id).is_some();
        data.push_subscriptions.retain(|_, s| &s.session_id != id);
        Ok(removed)
    }
    async fn delete_for_user(&self, user_id: &UserId) -> Result<u64, StoreError> {
        let mut data = self.0.lock().unwrap();
        let ids: BTreeSet<_> = data
            .sessions
            .values()
            .filter(|s| &s.user_id == user_id)
            .map(|s| s.id.clone())
            .collect();
        data.sessions.retain(|id, _| !ids.contains(id));
        data.push_subscriptions
            .retain(|_, s| !ids.contains(&s.session_id));
        Ok(ids.len() as u64)
    }
    async fn delete_expired(&self, now: UnixMillis) -> Result<u64, StoreError> {
        let mut data = self.0.lock().unwrap();
        let ids: BTreeSet<_> = data
            .sessions
            .values()
            .filter(|s| s.expires_at <= now)
            .map(|s| s.id.clone())
            .collect();
        data.sessions.retain(|id, _| !ids.contains(id));
        data.push_subscriptions
            .retain(|_, s| !ids.contains(&s.session_id));
        Ok(ids.len() as u64)
    }
}

#[async_trait::async_trait]
impl SignInLinkStore for FakeSignInLinkStore {
    async fn create(&self, link: &SignInLink) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        if data.sign_in_links.contains_key(&link.token_hash) {
            return Err(conflict("Sign-In Link exists"));
        }
        data.sign_in_links
            .insert(link.token_hash.clone(), link.clone());
        Ok(())
    }
    async fn consume(
        &self,
        token_hash: &str,
        kind: SignInLinkKind,
        now: UnixMillis,
    ) -> Result<Option<UserId>, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data
            .sign_in_links
            .get_mut(token_hash)
            .filter(|s| s.kind == kind && s.used_at.is_none() && s.expires_at > now)
        else {
            return Ok(None);
        };
        row.used_at = Some(now);
        Ok(Some(row.user_id.clone()))
    }
}

#[async_trait::async_trait]
impl HostStore for FakeHostStore {
    async fn register(
        &self,
        workspace_id: &WorkspaceId,
        name: &str,
        platform: &str,
        capabilities: &[String],
        at: UnixMillis,
    ) -> Result<Host, StoreError> {
        let mut data = self.0.lock().unwrap();
        let held = data
            .hosts
            .rows(workspace_id)
            .find(|h| h.name == name)
            .cloned();
        let mut row = held.unwrap_or_else(|| Host {
            id: HostId::generate(),
            workspace_id: workspace_id.clone(),
            name: name.to_string(),
            platform: platform.to_string(),
            capabilities: capabilities.to_vec(),
            last_seen_at: at,
            created_at: at,
        });
        row.platform = platform.to_string();
        row.capabilities = capabilities.to_vec();
        row.last_seen_at = at;
        data.hosts.put(workspace_id, row.id.clone(), row.clone());
        Ok(row)
    }
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &HostId,
    ) -> Result<Option<Host>, StoreError> {
        Ok(self.0.lock().unwrap().hosts.get(workspace_id, id).cloned())
    }
    async fn list(&self, workspace_id: &WorkspaceId) -> Result<Vec<Host>, StoreError> {
        let mut rows: Vec<_> = self
            .0
            .lock()
            .unwrap()
            .hosts
            .rows(workspace_id)
            .cloned()
            .collect();
        rows.sort_by_key(|h| (h.created_at, h.id.clone()));
        Ok(rows)
    }
    async fn touch(&self, id: &HostId, at: UnixMillis) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data.hosts.0.values_mut().find(|h| &h.id == id) else {
            return Ok(false);
        };
        row.last_seen_at = at;
        Ok(true)
    }
}

#[async_trait::async_trait]
impl PushSubscriptionStore for FakePushSubscriptionStore {
    async fn upsert(
        &self,
        subscription: &PushSubscription,
    ) -> Result<PushSubscription, StoreError> {
        let mut data = self.0.lock().unwrap();
        let mut row = subscription.clone();
        if let Some(held) = data
            .push_subscriptions
            .values()
            .find(|s| s.endpoint == row.endpoint)
        {
            row.id = held.id.clone();
            if held.session_id == row.session_id {
                row.created_at = held.created_at;
                row.last_sent_at = held.last_sent_at;
            }
        }
        data.push_subscriptions.insert(row.id.clone(), row.clone());
        Ok(row)
    }
    async fn list(&self, user_id: &UserId) -> Result<Vec<PushSubscription>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .push_subscriptions
            .values()
            .filter(|row| &row.user_id == user_id)
            .cloned()
            .collect())
    }
    async fn delete(&self, user_id: &UserId, id: &PushSubscriptionId) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        if data
            .push_subscriptions
            .get(id)
            .is_some_and(|row| &row.user_id == user_id)
        {
            data.push_subscriptions.remove(id);
            Ok(true)
        } else {
            Ok(false)
        }
    }
    async fn delete_by_endpoint(
        &self,
        user_id: &UserId,
        endpoint: &str,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let id = data
            .push_subscriptions
            .values()
            .find(|row| &row.user_id == user_id && row.endpoint == endpoint)
            .map(|row| row.id.clone());
        Ok(id.is_some_and(|id| data.push_subscriptions.remove(&id).is_some()))
    }
    async fn mark_sent(
        &self,
        user_id: &UserId,
        id: &PushSubscriptionId,
        at: UnixMillis,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data
            .push_subscriptions
            .get_mut(id)
            .filter(|row| &row.user_id == user_id)
        else {
            return Ok(false);
        };
        row.last_sent_at = Some(at);
        Ok(true)
    }
}

#[async_trait::async_trait]
impl crate::WorkspaceMemberStore for FakeWorkspaceMemberStore {
    async fn create_workspace(
        &self,
        workspace: &Workspace,
        manager: &crate::WorkspaceMember,
    ) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        if workspace.kind != crate::WorkspaceKind::Shared
            || workspace.user_id.is_some()
            || workspace.id != manager.workspace_id
            || manager.role != crate::WorkspaceRole::Manager
            || data.workspaces.contains_key(&workspace.id)
            || data.orgs.values().any(|o| o.workspace_id == workspace.id)
            || !data
                .users
                .get(&manager.person_id)
                .is_some_and(|u| !u.is_disabled())
        {
            return Err(conflict("a Shared Workspace needs its first Manager"));
        }
        data.workspaces
            .insert(workspace.id.clone(), workspace.clone());
        data.workspace_members.insert(
            (workspace.id.clone(), manager.person_id.clone()),
            manager.clone(),
        );
        Ok(())
    }
    async fn list(
        &self,
        workspace: &WorkspaceId,
    ) -> Result<Vec<crate::WorkspaceMember>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .workspace_members
            .values()
            .filter(|m| &m.workspace_id == workspace)
            .cloned()
            .collect())
    }
    async fn get(
        &self,
        workspace: &WorkspaceId,
        person: &UserId,
    ) -> Result<Option<crate::WorkspaceMember>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .workspace_members
            .get(&(workspace.clone(), person.clone()))
            .cloned())
    }
    async fn add(&self, member: &crate::WorkspaceMember) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        let key = (member.workspace_id.clone(), member.person_id.clone());
        if !data
            .workspaces
            .get(&member.workspace_id)
            .is_some_and(|w| w.kind == crate::WorkspaceKind::Shared)
            || !data
                .users
                .get(&member.person_id)
                .is_some_and(|u| !u.is_disabled())
            || data.workspace_members.contains_key(&key)
        {
            return Err(conflict("invalid membership"));
        }
        data.workspace_members.insert(key, member.clone());
        Ok(())
    }
    async fn change(&self, member: &crate::WorkspaceMember) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let key = (member.workspace_id.clone(), member.person_id.clone());
        let Some(old) = data.workspace_members.get(&key) else {
            return Ok(false);
        };
        if old.role == crate::WorkspaceRole::Manager
            && member.role != old.role
            && data
                .workspace_members
                .values()
                .filter(|m| {
                    m.workspace_id == member.workspace_id && m.role == crate::WorkspaceRole::Manager
                })
                .count()
                == 1
        {
            return Err(conflict("the last Manager cannot be demoted"));
        }
        data.workspace_members.insert(key, member.clone());
        Ok(true)
    }
    async fn remove(&self, workspace: &WorkspaceId, person: &UserId) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let key = (workspace.clone(), person.clone());
        let Some(old) = data.workspace_members.get(&key) else {
            return Ok(false);
        };
        if old.role == crate::WorkspaceRole::Manager
            && data
                .workspace_members
                .values()
                .filter(|m| &m.workspace_id == workspace && m.role == crate::WorkspaceRole::Manager)
                .count()
                == 1
        {
            return Err(conflict("the last Manager cannot be removed"));
        }
        data.workspace_members.remove(&key);
        Ok(true)
    }
}
