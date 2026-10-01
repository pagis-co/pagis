//! The Sign-In Links: one-use URLs whose secret trades for a Session.
//!
//! There are two kinds, and each route that spends a link spends its own
//! kind alone ([`SignInLinkKind`]):
//!
//! - **The start link**, `<local origin>/api/v1/sessions/link/<secret>`.
//!   The `pagis` binary of a Local Installation prints it at start, for a
//!   browser on the same machine. It lives one minute, and the daemon
//!   accepts it from that machine alone (ADR-0025).
//! - **A link of the Public Origin**, `<public origin>/sign-in#<secret>`
//!   (ADR-0028). The secret is in the fragment, so no proxy log and no
//!   `Referer` holds it. The Product App page at `/sign-in` posts it to
//!   `POST /api/v1/sessions/link`, so a GET spends nothing, and a message
//!   app that opens the link to make a preview does not spend it. Three
//!   things make one: a signed-in Person, for one more client of their
//!   own; an Administrator, to invite a Person; and `pagis pair`, on the
//!   machine of the installation.
//!
//! A link of the Public Origin travels with a QR code of its URL, so a
//! phone camera opens it. The routes that make one answer the code as an
//! SVG beside the URL, and `pagis pair` prints it as text.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use pagis_core::{
    CLIENT_LINK_LIFETIME_MS, INVITE_LINK_LIFETIME_MS, START_LINK_LIFETIME_MS, SignInLink,
    SignInLinkId, SignInLinkKind, SignInLinkStore, StoreError, UnixMillis, UserId,
};
use qrcode::QrCode;
use qrcode::render::{svg, unicode};
use serde::Serialize;
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::{Administrator, Tenant, hash_secret};
use crate::error::ApiError;
use crate::sessions::random_secret;

/// The path of the Product App page that spends a link of the Public
/// Origin.
pub const SIGN_IN_PAGE: &str = "/sign-in";

/// A link of the Public Origin that the store holds now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MintedLink {
    /// `<public origin>/sign-in#<secret>`.
    pub url: String,
    pub expires_at: UnixMillis,
}

/// A link of the Public Origin, as a route that makes one answers it.
#[derive(Debug, Serialize, ToSchema)]
pub struct SignInLinkDto {
    /// `<public origin>/sign-in#<secret>`. It is good for one use.
    pub url: String,
    /// When the link stops working, in Unix milliseconds.
    pub expires_at: i64,
    /// A QR code of the URL, as an SVG document.
    pub qr_svg: String,
}

impl SignInLinkDto {
    pub fn new(link: MintedLink) -> Result<Self, ApiError> {
        let qr_svg = qr_svg(&link.url).map_err(|error| {
            tracing::error!(%error, "the QR code of a sign-in link could not be made");
            ApiError::internal()
        })?;
        Ok(Self {
            url: link.url,
            expires_at: link.expires_at,
            qr_svg,
        })
    }
}

/// Write the start link and return the URL to open. The `pagis` binary
/// prints it: it is good for one minute and one use.
///
/// It is the one URL of the daemon that carries a secret in its path, and
/// it is bounded on both sides: one minute of life and one use. A link a
/// log or a browser history kept is already spent.
pub async fn mint_start_link(
    links: &dyn SignInLinkStore,
    user_id: &UserId,
    local_origin: &str,
    now: UnixMillis,
) -> Result<String, StoreError> {
    let secret = write_link(
        links,
        user_id,
        SignInLinkKind::Start,
        now + START_LINK_LIFETIME_MS,
        now,
    )
    .await?;
    Ok(format!(
        "{}/api/v1/sessions/link/{secret}",
        local_origin.trim_end_matches('/')
    ))
}

/// Write a link of the Public Origin for `user_id` that lives
/// `lifetime_ms`, and return its URL and its expiry.
pub async fn mint_public_origin_link(
    links: &dyn SignInLinkStore,
    user_id: &UserId,
    public_origin: &str,
    lifetime_ms: i64,
    now: UnixMillis,
) -> Result<MintedLink, StoreError> {
    let expires_at = now + lifetime_ms;
    let secret = write_link(
        links,
        user_id,
        SignInLinkKind::PublicOrigin,
        expires_at,
        now,
    )
    .await?;
    Ok(MintedLink {
        url: format!(
            "{}{SIGN_IN_PAGE}#{secret}",
            public_origin.trim_end_matches('/')
        ),
        expires_at,
    })
}

/// Write one link and return its secret. The record holds only the hash
/// of the secret.
async fn write_link(
    links: &dyn SignInLinkStore,
    user_id: &UserId,
    kind: SignInLinkKind,
    expires_at: UnixMillis,
    now: UnixMillis,
) -> Result<String, StoreError> {
    let secret = random_secret();
    links
        .create(&SignInLink {
            id: SignInLinkId::generate(),
            user_id: user_id.clone(),
            token_hash: hash_secret(&secret),
            kind,
            created_at: now,
            expires_at,
            used_at: None,
        })
        .await?;
    Ok(secret)
}

/// The QR code of `url` as an SVG document: dark modules on a light
/// square with its quiet zone, which every phone camera reads.
pub fn qr_svg(url: &str) -> Result<String, qrcode::types::QrError> {
    Ok(QrCode::new(url.as_bytes())?
        .render::<svg::Color<'_>>()
        .min_dimensions(240, 240)
        .build())
}

/// The QR code of `url` as lines of text for a terminal, two modules to
/// one character. The light modules are the block characters, so the
/// code reads on a dark terminal, as the `qrcode` crate documents.
pub fn qr_text(url: &str) -> Result<String, qrcode::types::QrError> {
    Ok(QrCode::new(url.as_bytes())?
        .render::<unicode::Dense1x2>()
        .dark_color(unicode::Dense1x2::Light)
        .light_color(unicode::Dense1x2::Dark)
        .build())
}

/// A link for an invite: good for seven days and one use. An
/// Administrator who creates a Person gets one, and so does an
/// Administrator who asks for a new one.
pub(crate) async fn invite(
    state: &AppState,
    user_id: &UserId,
    now: UnixMillis,
) -> Result<SignInLinkDto, ApiError> {
    let link = mint_public_origin_link(
        state.sign_in_links.as_ref(),
        user_id,
        &state.public_origin,
        INVITE_LINK_LIFETIME_MS,
        now,
    )
    .await?;
    SignInLinkDto::new(link)
}

/// Make a link for one more client of the signed-in Person: another
/// browser, or an app on a phone. It is good for five minutes and one
/// use, and it signs that client in as the same Person.
#[utoipa::path(
    post,
    path = "/api/v1/settings/sign-in-links",
    responses(
        (status = 201, body = SignInLinkDto),
        (status = 401, body = crate::error::ErrorBody),
    )
)]
pub async fn make_client_link(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<(StatusCode, Json<SignInLinkDto>), ApiError> {
    let link = mint_public_origin_link(
        state.sign_in_links.as_ref(),
        &tenant.user_id,
        &state.public_origin,
        CLIENT_LINK_LIFETIME_MS,
        state.clock.now_ms(),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(SignInLinkDto::new(link)?)))
}

/// Make a new invite for a Person: a link for their next Session, good
/// for seven days and one use. An Administrator asks for one when the
/// invite of the account expired, or when the Person has no Session
/// left.
#[utoipa::path(
    post,
    path = "/api/v1/administration/people/{user_id}/sign-in-links",
    params(("user_id" = String, Path, description = "The person")),
    responses(
        (status = 201, body = SignInLinkDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody, description = "The account is disabled"),
    )
)]
pub async fn make_invite_link(
    State(state): State<Arc<AppState>>,
    _administrator: Administrator,
    Path(user_id): Path<String>,
) -> Result<(StatusCode, Json<SignInLinkDto>), ApiError> {
    let person = crate::administration::person_of(&state, &user_id).await?;
    // A disabled account signs in to nothing, so a link for it would be
    // refused at the trade. The Administrator enables it first.
    if person.is_disabled() {
        return Err(ApiError::validation(
            "that account is disabled; enable it before you invite the person",
        ));
    }
    let link = invite(&state, &person.id, state.clock.now_ms()).await?;
    Ok((StatusCode::CREATED, Json(link)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_qr_code_is_an_svg_document() {
        let svg = qr_svg("https://pagis.example/sign-in#abc").unwrap();
        assert!(svg.contains("<svg"), "{svg}");
        assert!(svg.trim_end().ends_with("</svg>"), "{svg}");
    }

    /// The text code is square: each line holds two rows of modules,
    /// so a code of `n` modules with its quiet zone is `n` characters
    /// wide and about `n / 2` lines high.
    #[test]
    fn the_terminal_code_is_lines_of_block_characters() {
        let text = qr_text("https://pagis.example/sign-in#abc").unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let width = lines[0].chars().count();
        assert!(lines.iter().all(|line| line.chars().count() == width));
        assert_eq!(lines.len(), width.div_ceil(2));
        assert!(text.contains('\u{2588}'), "{text}");
    }
}
