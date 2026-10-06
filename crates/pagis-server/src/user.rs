//! The signed-in person.
//!
//! The name lives on the `users` record. The daemon writes
//! `shared/user.md` from it, because every agent reads that file; the
//! file is a copy of the record and not the record itself.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use pagis_core::{User, UserRole};
use serde::Serialize;
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
}

impl UserDto {
    /// The person as the shell reads them. Only an Administrator learns
    /// where the Administration Interface answers.
    pub fn new(
        user: &User,
        administration: &AdministrationAddress,
        capture: &pagis_core::CaptureSetting,
    ) -> Self {
        UserDto {
            id: user.id.to_string(),
            name: user.name.clone(),
            email: user.email.clone(),
            role: user.role.as_str().to_string(),
            administration: (user.role == UserRole::Administrator).then(|| administration.clone()),
            model_request_capture_days: capture.is_enabled().then(|| capture.retention_days()),
        }
    }
}

/// Where a browser reaches the Administration Interface (ADR-0024).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct AdministrationAddress {
    /// The origin of the administration port, such as
    /// `http://127.0.0.1:4401`.
    pub origin: String,
    /// True where the port binds loopback. Only the machine that runs
    /// the daemon reaches it then, and an administrator on another
    /// machine reaches it through an SSH tunnel to the same port.
    pub loopback: bool,
}

impl AdministrationAddress {
    /// The address of the listener that binds `addr`. A wildcard bind
    /// has no one address, so the origin names loopback, which reaches
    /// it from the machine itself.
    pub fn of(addr: std::net::SocketAddr) -> Self {
        let host = if addr.ip().is_unspecified() {
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        } else {
            addr.ip()
        };
        AdministrationAddress {
            origin: format!("http://{}", std::net::SocketAddr::new(host, addr.port())),
            loopback: addr.ip().is_loopback(),
        }
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
    Ok(Json(UserDto::new(
        &user,
        &state.administration,
        &state.capture,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_loopback_listener_is_reached_at_loopback() {
        let address = AdministrationAddress::of("127.0.0.1:4401".parse().unwrap());
        assert_eq!(address.origin, "http://127.0.0.1:4401");
        assert!(address.loopback);
    }

    #[test]
    fn a_private_listener_is_reached_at_its_own_address() {
        let address = AdministrationAddress::of("10.0.0.5:9443".parse().unwrap());
        assert_eq!(address.origin, "http://10.0.0.5:9443");
        assert!(!address.loopback);
    }

    #[test]
    fn a_wildcard_listener_names_loopback() {
        let address = AdministrationAddress::of("0.0.0.0:4401".parse().unwrap());
        assert_eq!(address.origin, "http://127.0.0.1:4401");
        assert!(!address.loopback);
    }

    #[test]
    fn only_an_administrator_learns_the_address() {
        let address = AdministrationAddress::of("127.0.0.1:4401".parse().unwrap());
        let mut person = User::new(
            pagis_core::OrgId::from("o".to_string()),
            UserRole::Administrator,
            0,
        );
        assert_eq!(
            UserDto::new(&person, &address, &pagis_core::CaptureSetting::default()).administration,
            Some(address.clone())
        );
        person.role = UserRole::Member;
        assert_eq!(
            UserDto::new(&person, &address, &pagis_core::CaptureSetting::default()).administration,
            None
        );
    }

    #[test]
    fn the_rendered_file_names_the_person() {
        let file = render_user_file("Ada");
        assert!(file.contains("The user's name: Ada."));
    }
}
