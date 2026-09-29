//! The call half of the carrier over SIP (ADR-0020). This is the one
//! place `rsipstack` appears. It registers outward over TLS to the
//! registrar the SIP credential names, so the daemon needs no public
//! ingress. Nothing in it belongs to one carrier: Telnyx, Twilio and
//! Plivo each accept an outward `REGISTER` from a credential. One
//! socket serves every number of the carrier: calls go out and come in
//! on it, and each inbound `INVITE` names the number that was dialed.
//! The media leg beside each dialog is [`crate::sip_media`].
//!
//! What the pure-SIP path cannot do, the transport declares absent:
//! answering-machine detection is a call-control feature, and the offer
//! carries G.711 only, so there is no wideband audio.

use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use rsipstack::EndpointBuilder;
use rsipstack::dialog::authenticate::Credential;
use rsipstack::dialog::dialog_layer::DialogLayer;
use rsipstack::dialog::invitation::InviteOption;
use rsipstack::dialog::invite_dialog::InviteDialog;
use rsipstack::dialog::registration::Registration;
use rsipstack::sip::prelude::HeadersExt;
use rsipstack::sip::{Header, Method, StatusCode, ToTypedHeader};
use rsipstack::transaction::endpoint::Endpoint;
use rsipstack::transaction::transaction::Transaction;
use rsipstack::transport::tls::{TlsConfig, TlsConnection};
use rsipstack::transport::{SipAddr, TransportLayer};
use rustls::client::danger::ServerCertVerifier;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::leg::MediaLeg;
use crate::sip_media::{SipLeg, SipMedia};
use crate::transport::{
    Answer, CallTransport, IncomingCall, Line, Opened, Refusal, SipCredential,
    TransportCapabilities, TransportError, TransportErrorCode,
};

/// TLS signaling, and nothing else (ADR-0020).
const SIP_TLS_PORT: u16 = 5061;
const USER_AGENT: &str = concat!("pagis/", env!("CARGO_PKG_VERSION"));
/// How many calls may wait for the endpoint task to answer them.
const INCOMING_DEPTH: usize = 4;

pub struct SipCallTransport {
    verifier: Arc<dyn ServerCertVerifier>,
}

impl SipCallTransport {
    /// Build the transport. It trusts the Mozilla root store, the same
    /// one the REST client trusts, so the registrar's certificate is
    /// checked and never waved through.
    pub fn new() -> Result<Self, TransportError> {
        let roots = rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        let verifier = rustls::client::WebPkiServerVerifier::builder(Arc::new(roots))
            .build()
            .map_err(|error| {
                tracing::error!(%error, "the TLS verifier failed to build");
                TransportError(TransportErrorCode::Unreachable)
            })?;
        Ok(Self { verifier })
    }
}

#[async_trait]
impl CallTransport for SipCallTransport {
    fn capabilities(&self) -> TransportCapabilities {
        TransportCapabilities {
            // RFC 2833 events on the RTP leg.
            send_dtmf: true,
            // A call-control feature; the SIP path does not have it.
            answering_machine_detection: false,
            // The offer carries PCMU and PCMA only.
            wideband_audio: false,
            // Signaling goes outward, so the carrier needs no route in.
            public_ingress_required: false,
        }
    }

    async fn open(&self, credential: &SipCredential) -> Result<Opened, TransportError> {
        let registrar = resolve(credential.domain()).await?;
        let cancel = CancellationToken::new();
        let transport_layer = TransportLayer::new(cancel.clone());
        // The connection is keyed by the resolved address, which is the
        // address the registration is pinned to below, so every
        // `REGISTER` travels on this one socket.
        let remote = SipAddr::from(registrar);
        let remote = SipAddr {
            r#type: Some(rsipstack::sip::Transport::Tls),
            ..remote
        };
        let tls = TlsConfig {
            sni_hostname: Some(credential.domain().to_string()),
            ..TlsConfig::default()
        };
        let connection = TlsConnection::connect(
            &remote,
            Some(&tls),
            Some(Arc::clone(&self.verifier)),
            Some(cancel.child_token()),
        )
        .await
        .map_err(|error| {
            tracing::warn!(%error, domain = credential.domain(), "connecting to the registrar failed");
            TransportError(TransportErrorCode::Unreachable)
        })?;
        transport_layer.add_connection(connection.into());
        let endpoint = EndpointBuilder::new()
            .with_user_agent(USER_AGENT)
            .with_transport_layer(transport_layer)
            .with_cancel_token(cancel.clone())
            .with_allows(vec![
                Method::Invite,
                Method::Ack,
                Method::Bye,
                Method::Cancel,
                Method::Options,
                Method::Info,
                Method::Update,
            ])
            .build();
        let serving = endpoint.inner.clone();
        tokio::spawn(async move {
            if let Err(error) = serving.serve().await {
                tracing::warn!(%error, "the SIP endpoint stopped");
            }
        });
        let sip_credential = Credential {
            username: credential.username().to_string(),
            password: credential.expose_password().to_string(),
            realm: None,
        };
        let mut registration =
            Registration::new(endpoint.inner.clone(), Some(sip_credential.clone()));
        registration.outbound_proxy = Some(registrar);
        let server = rsipstack::sip::Uri::try_from(
            format!("sip:{};transport=tls", credential.domain()).as_str(),
        )
        .map_err(|error| {
            tracing::warn!(%error, domain = credential.domain(), "the registrar domain is not a SIP host");
            TransportError(TransportErrorCode::Refused)
        })?;
        let dialog_layer = Arc::new(DialogLayer::new(endpoint.inner.clone()));
        let local = dialog_layer
            .build_local_contact(Some(credential.username().to_string()), None)
            .map_err(|error| {
                tracing::error!(%error, "no local address for the REGISTER Contact");
                TransportError(TransportErrorCode::Unreachable)
            })?;
        registration.contact = Some(register_contact(local));
        let transactions = endpoint.incoming_transactions().map_err(|error| {
            tracing::error!(%error, "the SIP endpoint gives no transactions");
            TransportError(TransportErrorCode::Unreachable)
        })?;
        let (incoming_tx, incoming) = mpsc::channel(INCOMING_DEPTH);
        tokio::spawn(serve_incoming(
            transactions,
            Arc::clone(&dialog_layer),
            credential.username().to_string(),
            incoming_tx,
        ));
        Ok(Opened {
            line: Box::new(TelnyxLine {
                endpoint,
                registration,
                server,
                dialog_layer,
                credential: sip_credential,
                domain: credential.domain().to_string(),
                registrar: remote,
            }),
            incoming,
        })
    }
}

/// One TLS socket to the registrar and the registration that runs on
/// it. Dropping it cancels the endpoint, which closes the socket.
struct TelnyxLine {
    endpoint: Endpoint,
    registration: Registration,
    server: rsipstack::sip::Uri,
    dialog_layer: Arc<DialogLayer>,
    credential: Credential,
    domain: String,
    registrar: SipAddr,
}

impl TelnyxLine {
    async fn send(&mut self, expires: u32) -> Result<u32, TransportError> {
        let response = self
            .registration
            .register(self.server.clone(), Some(expires))
            .await
            .map_err(|error| {
                tracing::warn!(%error, "REGISTER did not complete");
                TransportError(TransportErrorCode::Unreachable)
            })?;
        match response.status_code {
            StatusCode::OK => Ok(granted_expiry(&response).unwrap_or(expires)),
            StatusCode::Unauthorized
            | StatusCode::Forbidden
            | StatusCode::ProxyAuthenticationRequired => {
                Err(TransportError(TransportErrorCode::Unauthorized))
            }
            other => {
                tracing::warn!(status = %other, "the registrar refused the registration");
                Err(TransportError(TransportErrorCode::Refused))
            }
        }
    }
}

#[async_trait]
impl Line for TelnyxLine {
    async fn register(
        &mut self,
        expires: std::time::Duration,
    ) -> Result<std::time::Duration, TransportError> {
        let requested = u32::try_from(expires.as_secs()).unwrap_or(u32::MAX);
        let granted = self.send(requested).await?;
        Ok(std::time::Duration::from_secs(u64::from(granted)))
    }

    async fn unregister(&mut self) -> Result<(), TransportError> {
        self.send(0).await.map(|_| ())
    }

    async fn dial(&mut self, from_e164: &str, to_e164: &str) -> Result<MediaLeg, TransportError> {
        let media = SipMedia::new();
        let offer = media.offer().await?;
        let uri = |user: &str| {
            rsipstack::sip::Uri::try_from(format!("sip:{user}@{}", self.domain).as_str()).map_err(
                |error| {
                    tracing::warn!(%error, "not a SIP address");
                    TransportError(TransportErrorCode::Refused)
                },
            )
        };
        let contact = self
            .dialog_layer
            .build_local_contact(Some(self.credential.username.clone()), None)
            .map_err(|error| {
                tracing::warn!(%error, "no local contact for the INVITE");
                TransportError(TransportErrorCode::Unreachable)
            })?;
        let option = InviteOption {
            caller: uri(from_e164)?,
            callee: uri(to_e164)?,
            destination: Some(self.registrar.clone()),
            content_type: Some("application/sdp".to_string()),
            offer: Some(offer.into_bytes()),
            contact,
            credential: Some(self.credential.clone()),
            ..InviteOption::default()
        };
        let (states_tx, states) = self.dialog_layer.new_dialog_state_channel();
        let (dialog, invite) = self
            .dialog_layer
            .do_invite_async(option, states_tx)
            .map_err(|error| {
                tracing::warn!(%error, "the INVITE did not go out");
                TransportError(TransportErrorCode::Unreachable)
            })?;
        Ok(SipLeg::start(
            media,
            dialog,
            Arc::clone(&self.dialog_layer),
            states,
            None,
            Some(invite),
        ))
    }
}

impl Drop for TelnyxLine {
    fn drop(&mut self) {
        self.endpoint.shutdown();
    }
}

/// The Contact a `REGISTER` binds, from the socket's own address:
/// `sip:user@host:port;transport=tls`. The registrar sends every
/// inbound `INVITE` to this address, and without `transport=tls` it
/// sends them over UDP, which never reaches a line behind NAT: the
/// carrier logs a 408 and the daemon sees nothing. rsipstack swaps the
/// host for the public address the registrar reports and keeps the
/// parameters.
pub fn register_contact(mut local: rsipstack::sip::Uri) -> rsipstack::sip::typed::Contact {
    local.scheme = Some(rsipstack::sip::Scheme::Sip);
    if !local
        .params
        .iter()
        .any(|param| matches!(param, rsipstack::sip::Param::Transport(_)))
    {
        local.params.push(rsipstack::sip::Param::Transport(
            rsipstack::sip::Transport::Tls,
        ));
    }
    rsipstack::sip::typed::Contact {
        display_name: None,
        uri: local,
        params: Vec::new(),
    }
}

/// Every transaction the socket delivers: a request inside a dialog
/// goes to that dialog, a new `INVITE` becomes an [`IncomingCall`], and
/// the rest gets a short answer.
async fn serve_incoming(
    mut transactions: rsipstack::transaction::TransactionReceiver,
    dialog_layer: Arc<DialogLayer>,
    username: String,
    incoming: mpsc::Sender<IncomingCall>,
) {
    while let Some(mut tx) = transactions.recv().await {
        if let Some(mut dialog) = dialog_layer.match_dialog(&tx) {
            if let Err(error) = dialog.handle(&mut tx).await {
                tracing::debug!(%error, method = %tx.original.method, "an in-dialog request failed");
            }
            continue;
        }
        match tx.original.method {
            Method::Invite => {
                if let Err(error) = offer_call(&dialog_layer, &username, &incoming, tx).await {
                    tracing::warn!(%error, "an INVITE was not offered");
                }
            }
            Method::Options => {
                let _ = tx.reply(StatusCode::OK).await;
            }
            Method::Ack | Method::Cancel => {
                // A late ACK or CANCEL for a dialog that is gone.
            }
            _ => {
                let _ = tx.reply(StatusCode::MethodNotAllowed).await;
            }
        }
    }
}

/// A new `INVITE`: make the server dialog, drive its transaction, and
/// hand the call to the endpoint task with the number that was dialed.
/// The error is boxed because `rsipstack::Error` is more than 128 bytes
/// wide, and an unboxed one makes every `Ok` on this path pay for it
/// (`clippy::result_large_err`).
async fn offer_call(
    dialog_layer: &Arc<DialogLayer>,
    username: &str,
    incoming: &mpsc::Sender<IncomingCall>,
    mut tx: Transaction,
) -> Result<(), Box<rsipstack::Error>> {
    let (states_tx, states) = dialog_layer.new_dialog_state_channel();
    let contact = dialog_layer
        .build_local_contact(Some(username.to_string()), None)
        .ok();
    let dialog = dialog_layer.get_or_create_server_invite(&tx, states_tx, None, contact)?;
    let dialed_e164 = dialed_e164(&tx.original, username);
    let from_e164 = caller_e164(&tx.original);
    let mut driven = dialog.clone();
    tokio::spawn(async move {
        if let Err(error) = driven.handle(&mut tx).await {
            tracing::debug!(%error, "the INVITE transaction ended badly");
        }
    });
    let call = IncomingCall {
        dialed_e164,
        from_e164,
        answer: Box::new(TelnyxAnswer {
            dialog: dialog.clone(),
            dialog_layer: Arc::clone(dialog_layer),
            states,
        }),
    };
    if incoming.try_send(call).is_err() {
        tracing::warn!("nobody takes calls on this line; answering 480");
        dialog.reject(Some(StatusCode::TemporarilyUnavailable), None)?;
    }
    Ok(())
}

/// The number the Remote Party dialed, in E.164: the user part of the
/// Request-URI. A registrar that sends the `INVITE` to the registered
/// Contact puts the SIP username there instead, and then the user part
/// of `To` names the number. `None` when the number is missing or is
/// not E.164; no `+` is added, because a national number with a `+` in
/// front is another country's number.
pub fn dialed_e164(request: &rsipstack::sip::Request, username: &str) -> Option<String> {
    let user = request.uri.user()?;
    if user != username {
        return pagis_core::normalize_e164(user);
    }
    let to = request.to_header().ok()?.typed().ok()?;
    pagis_core::normalize_e164(&to.uri.auth?.user)
}

/// The caller's number as the Call record wants it: the user part of
/// `From`, with the `+` a carrier often leaves out.
fn caller_e164(request: &rsipstack::sip::Request) -> String {
    let user = request
        .from_header()
        .ok()
        .and_then(|from| from.typed().ok())
        .and_then(|from| from.uri.auth.map(|auth| auth.user))
        .unwrap_or_default();
    if !user.is_empty() && !user.starts_with('+') && user.bytes().all(|byte| byte.is_ascii_digit())
    {
        format!("+{user}")
    } else {
        user
    }
}

struct TelnyxAnswer {
    dialog: InviteDialog,
    dialog_layer: Arc<DialogLayer>,
    states: rsipstack::dialog::dialog::DialogStateReceiver,
}

#[async_trait]
impl Answer for TelnyxAnswer {
    async fn accept(self: Box<Self>) -> Result<MediaLeg, TransportError> {
        let offer = String::from_utf8_lossy(&self.dialog.initial_request().body).into_owned();
        let media = SipMedia::new();
        let Some((answer, negotiated)) = media.answer(&offer).await? else {
            tracing::warn!("the offer cannot be answered; answering 488");
            let _ = self
                .dialog
                .reject(Some(StatusCode::NotAcceptableHere), None);
            return Err(TransportError(TransportErrorCode::Refused));
        };
        self.dialog
            .accept(
                Some(vec![Header::ContentType("application/sdp".into())]),
                Some(answer.into_bytes()),
            )
            .map_err(|error| {
                tracing::warn!(%error, "the 200 OK did not go out");
                TransportError(TransportErrorCode::Unreachable)
            })?;
        Ok(SipLeg::start(
            media,
            self.dialog,
            self.dialog_layer,
            self.states,
            Some(negotiated),
            None,
        ))
    }

    async fn reject(self: Box<Self>, refusal: Refusal) {
        let status = StatusCode::from(refusal.status());
        if let Err(error) = self.dialog.reject(Some(status.clone()), None) {
            tracing::warn!(%error, %status, "the refusal did not go out");
        }
        self.dialog_layer.remove_dialog(&self.dialog.id());
    }
}

/// The expiry the registrar granted: the `Expires` header, or the
/// `expires` parameter of a Contact in the answer.
fn granted_expiry(response: &rsipstack::sip::Response) -> Option<u32> {
    if let Some(seconds) = response
        .expires_header()
        .and_then(|expires| expires.value().trim().parse().ok())
    {
        return Some(seconds);
    }
    response
        .contact_headers()
        .into_iter()
        .filter_map(|contact| contact.typed().ok())
        .find_map(|contact| contact.expires())
}

async fn resolve(domain: &str) -> Result<SocketAddr, TransportError> {
    tokio::net::lookup_host((domain, SIP_TLS_PORT))
        .await
        .ok()
        .and_then(|mut addresses| addresses.next())
        .ok_or_else(|| {
            tracing::warn!(domain, "the registrar did not resolve");
            TransportError(TransportErrorCode::Unreachable)
        })
}
