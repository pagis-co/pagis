//! The signed-in person.
//!
//! The name lives on the `users` record. The daemon writes
//! `shared/user.md` from it, because every agent reads that file; the
//! file is a copy of the record and not the record itself.

use std::sync::Arc;

use async_trait::async_trait;
use axum::Json;
use axum::extract::State;
use pagis_core::{StoreError, User, UserRole};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::Tenant;
use crate::error::ApiError;

/// The memory file that carries the name to the agents.
pub const USER_FILE: &str = "shared/user.md";

/// The sentence that carries the name. The file is written from the
/// record, so nothing reads it back.
const NAME_PREFIX: &str = "The user's name: ";
const NAME_SUFFIX: &str = ". Address the user by this name.";

/// The content of `shared/user.md` for one name.
pub fn render_user_file(name: &str) -> String {
    format!("# User\n\n{NAME_PREFIX}{name}{NAME_SUFFIX}\n")
}

/// What the shell knows about the person it serves.
#[derive(Debug, Serialize, ToSchema)]
pub struct UserDto {
    pub id: String,
    /// The name the agents call the person by; `null` until onboarding
    /// records one.
    pub name: Option<String>,
    /// The address the person signs in with; `null` for the seeded
    /// person of a local installation.
    pub email: Option<String>,
    /// `administrator` or `member`.
    pub role: String,
    /// Where the Administration Interface answers, for an Administrator.
    /// The product links to it and draws no installation setting of its
    /// own. `null` for a Member.
    pub administration: Option<AdministrationAddress>,
    /// How many days Pagis keeps a copy of each model request, while an
    /// Administrator has Model Request Capture on (ADR-0031). `null`
    /// while it is off. Every Person learns it, because the copy holds
    /// their own requests.
    pub model_request_capture_days: Option<u32>,
    /// The links the operator names in the Server configuration.
    pub product_app_links: Vec<ConfiguredLink>,
    /// The Org name and the menu entries the server supplies.
    pub org: OrgDto,
    /// The one notice the server chooses for this Person.
    pub notice: Option<PersonNoticeDto>,
}

impl UserDto {
    /// The person as the shell reads them. Only an Administrator learns
    /// where the Administration Interface answers.
    pub fn new(
        user: &User,
        administration: &AdministrationAddress,
        capture: &pagis_core::CaptureSetting,
        product_app_links: &[ConfiguredLink],
        org: OrgDto,
        notice: Option<PersonNoticeDto>,
    ) -> Self {
        UserDto {
            id: user.id.to_string(),
            name: user.name.clone(),
            email: user.email.clone(),
            role: user.role.as_str().to_string(),
            administration: (user.role == UserRole::Administrator).then(|| administration.clone()),
            model_request_capture_days: capture.is_enabled().then(|| capture.retention_days()),
            product_app_links: product_app_links.to_vec(),
            org,
            notice,
        }
    }
}

/// The one notice the server chooses for a Person.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PersonNoticeDto {
    pub tone: Option<NoticeTone>,
    pub label: String,
    #[schema(value_type = String, max_length = 120)]
    pub text: NoticeText,
    /// The link opens in the same tab.
    pub link: Option<ConfiguredLink>,
}

/// The Badge tone of a Person notice. No tone uses neutral.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NoticeTone {
    Neutral,
    Waiting,
}

/// Notice text holds at most 120 characters.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(try_from = "String")]
#[schema(value_type = String)]
pub struct NoticeText(String);

impl TryFrom<String> for NoticeText {
    type Error = &'static str;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        if text.chars().count() > 120 {
            return Err("notice text must hold at most 120 characters");
        }
        Ok(Self(text))
    }
}

/// The notice for a Person, read on each request.
#[async_trait]
pub trait PersonNotice: Send + Sync {
    async fn notice(&self, person: &User) -> Result<Option<PersonNoticeDto>, StoreError>;
}

/// The daemon supplies no Person notice.
pub struct NoPersonNotice;

#[async_trait]
impl PersonNotice for NoPersonNotice {
    async fn notice(&self, _person: &User) -> Result<Option<PersonNoticeDto>, StoreError> {
        Ok(None)
    }
}

/// The Org that the Product App shows in its head.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct OrgDto {
    pub name: String,
    pub entries: Vec<OrgMenuEntry>,
}

/// One link in the Org menu, supplied by the server or an Embedder.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct OrgMenuEntry {
    pub label: String,
    /// The link opens in the same tab.
    pub url: String,
    pub detail: Option<String>,
    pub current: bool,
}

/// The Org menu entries for a Person, read on each request.
#[async_trait]
pub trait OrgMenu: Send + Sync {
    async fn entries(&self, person: &User) -> Result<Vec<OrgMenuEntry>, StoreError>;
}

/// The daemon supplies no Org menu entries.
pub struct NoOrgMenu;

#[async_trait]
impl OrgMenu for NoOrgMenu {
    async fn entries(&self, _person: &User) -> Result<Vec<OrgMenuEntry>, StoreError> {
        Ok(Vec::new())
    }
}

/// Read the Person's Org for the user and sign-in answers.
pub(crate) async fn user_dto(state: &AppState, user: &User) -> Result<UserDto, ApiError> {
    let org = state
        .orgs
        .list()
        .await?
        .into_iter()
        .find(|org| org.id == user.org_id)
        .ok_or_else(|| ApiError::not_found("the Org"))?;
    let administration = AdministrationAddress {
        sections: state.administration_sections.offered(),
        links: state.administration_sections.links(),
        ..state.administration.clone()
    };
    Ok(UserDto::new(
        user,
        &administration,
        &state.capture,
        &state.product_app_links,
        OrgDto {
            name: org.name,
            entries: state.org_menu.entries(user).await?,
        },
        state.person_notice.notice(user).await?,
    ))
}

/// One link that the Server configuration names: in the account menu of
/// the Product App, or in the navigation of the Administration Interface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ConfiguredLink {
    /// The text of the link, at most 40 characters.
    #[schema(max_length = 40)]
    pub label: String,
    /// A valid https URL. The link opens in the same tab.
    pub url: String,
}

/// Where a browser reaches the Administration Interface (ADR-0024).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct AdministrationAddress {
    /// The Public Origin with a page path prefix, or the Administration
    /// Port origin with no prefix, such as `http://127.0.0.1:4401`.
    pub origin: String,
    /// The page path prefix. Empty serves the page at the origin root.
    pub path_prefix: String,
    /// True for a direct address whose port binds loopback. A proxy
    /// address needs no SSH tunnel to the Administration Port.
    pub loopback: bool,
    /// The offered sections, in navigation order. Hosts and Restart have no view.
    pub sections: Vec<&'static str>,
    /// The links the Server configuration names, in navigation order.
    #[schema(max_items = 3)]
    pub links: Vec<ConfiguredLink>,
}

impl AdministrationAddress {
    /// A prefix names a proxy page under the Public Origin. With no
    /// prefix, use the listener address. A wildcard bind names loopback.
    pub fn of(addr: std::net::SocketAddr, public_origin: &str, path_prefix: &str) -> Self {
        let host = if addr.ip().is_unspecified() {
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        } else {
            addr.ip()
        };
        AdministrationAddress {
            origin: if path_prefix.is_empty() {
                format!("http://{}", std::net::SocketAddr::new(host, addr.port()))
            } else {
                public_origin.to_string()
            },
            path_prefix: path_prefix.to_string(),
            loopback: path_prefix.is_empty() && addr.ip().is_loopback(),
            sections: Vec::new(),
            links: Vec::new(),
        }
    }

    /// The full address of the Administration Interface, with no final slash.
    pub fn url(&self) -> String {
        format!("{}{}", self.origin, self.path_prefix)
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/user",
    responses(
        (status = 200, body = UserDto),
        (status = 401, body = crate::error::ErrorBody),
    )
)]
pub async fn get_user(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<Json<UserDto>, ApiError> {
    let user = state
        .users
        .get(&tenant.user_id)
        .await?
        .ok_or_else(|| ApiError::not_found("the person"))?;
    Ok(Json(user_dto(&state, &user).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notice_text_refuses_more_than_120_characters() {
        assert!(NoticeText::try_from("x".repeat(121)).is_err());
        assert!(serde_json::from_value::<NoticeText>(serde_json::json!("x".repeat(121))).is_err());
        let text = NoticeText::try_from("é".repeat(120)).unwrap();
        assert_eq!(serde_json::to_value(text).unwrap(), "é".repeat(120));
    }

    #[test]
    fn a_loopback_listener_is_reached_at_loopback() {
        let address = AdministrationAddress::of(
            "127.0.0.1:4401".parse().unwrap(),
            "https://pagis.example.net",
            "",
        );
        assert_eq!(address.origin, "http://127.0.0.1:4401");
        assert!(address.loopback);
        assert_eq!(address.path_prefix, "");
        assert_eq!(address.url(), "http://127.0.0.1:4401");
    }

    #[test]
    fn a_private_listener_is_reached_at_its_own_address() {
        let address = AdministrationAddress::of(
            "10.0.0.5:9443".parse().unwrap(),
            "https://pagis.example.net",
            "",
        );
        assert_eq!(address.origin, "http://10.0.0.5:9443");
        assert!(!address.loopback);
    }

    #[test]
    fn a_wildcard_listener_names_loopback() {
        let address = AdministrationAddress::of(
            "0.0.0.0:4401".parse().unwrap(),
            "https://pagis.example.net",
            "",
        );
        assert_eq!(address.origin, "http://127.0.0.1:4401");
        assert!(!address.loopback);
    }

    #[test]
    fn a_prefixed_address_names_the_public_origin_and_needs_no_tunnel() {
        let address = AdministrationAddress::of(
            "127.0.0.1:4401".parse().unwrap(),
            "https://pagis.example.net",
            "/team/admin",
        );
        assert_eq!(address.origin, "https://pagis.example.net");
        assert_eq!(address.path_prefix, "/team/admin");
        assert_eq!(address.url(), "https://pagis.example.net/team/admin");
        assert!(!address.loopback);
    }

    #[test]
    fn only_an_administrator_learns_the_address() {
        let address = AdministrationAddress::of(
            "127.0.0.1:4401".parse().unwrap(),
            "https://pagis.example.net",
            "",
        );
        let mut person = User::new(
            pagis_core::OrgId::from("o".to_string()),
            UserRole::Administrator,
            0,
        );
        assert_eq!(
            UserDto::new(
                &person,
                &address,
                &pagis_core::CaptureSetting::default(),
                &[],
                OrgDto {
                    name: "Acme".into(),
                    entries: Vec::new()
                },
                None,
            )
            .administration,
            Some(address.clone())
        );
        person.role = UserRole::Member;
        assert_eq!(
            UserDto::new(
                &person,
                &address,
                &pagis_core::CaptureSetting::default(),
                &[],
                OrgDto {
                    name: "Acme".into(),
                    entries: Vec::new()
                },
                None,
            )
            .administration,
            None
        );
    }

    #[test]
    fn the_user_answer_carries_the_org() {
        let person = User::new(
            pagis_core::OrgId::from("o".to_string()),
            UserRole::Member,
            0,
        );
        let address =
            AdministrationAddress::of("127.0.0.1:4401".parse().unwrap(), "http://localhost", "");
        let answer = UserDto::new(
            &person,
            &address,
            &pagis_core::CaptureSetting::default(),
            &[],
            OrgDto {
                name: "Acme".into(),
                entries: Vec::new(),
            },
            None,
        );
        let value = serde_json::to_value(answer).unwrap();
        assert_eq!(
            value["org"],
            serde_json::json!({"name": "Acme", "entries": []})
        );
    }

    #[test]
    fn the_rendered_file_names_the_person() {
        let file = render_user_file("Ada");
        assert!(file.contains("The user's name: Ada."));
    }
}
