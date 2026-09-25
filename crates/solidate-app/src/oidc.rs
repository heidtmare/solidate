//! OpenID Connect sign-in: authorization code flow with PKCE.
//!
//! [`Oidc::begin`] returns the provider's authorization URL and a [`PendingLogin`]
//! that the front end keeps until the callback. [`Oidc::finish`] exchanges the
//! code and verifies the ID token (signature, issuer, audience, expiry, nonce,
//! access token hash). [`App::oidc_login`] maps the verified [`OidcIdentity`] to a
//! user.
//!
//! User mapping: an identity already linked to a user signs in as that user.
//! Otherwise the identity needs a verified email; it is linked to the user with
//! that email, or a user without a password is created. New users have no tenant
//! memberships.

use std::sync::RwLock;

use openidconnect::core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata};
use openidconnect::{
    AccessTokenHash, AuthorizationCode, ClaimsVerificationError, ClientId, ClientSecret, CsrfToken, EndpointMaybeSet,
    EndpointNotSet, EndpointSet, IssuerUrl, Nonce, OAuth2TokenResponse, PkceCodeChallenge, PkceCodeVerifier,
    RedirectUrl, Scope, TokenResponse, reqwest,
};
use solidate_db::User;
use subtle::ConstantTimeEq;

use crate::App;
use crate::error::{AppError, Result, invalid};

/// Path of the callback the web front end serves.
pub const CALLBACK_PATH: &str = "/login/oidc/callback";

/// Settings for one OpenID Connect provider.
#[derive(Debug, Clone)]
pub struct OidcConfig {
    pub issuer: String,
    pub client_id: String,
    /// `None` for public clients (PKCE only).
    pub client_secret: Option<String>,
    /// Absolute callback URL registered with the provider.
    pub redirect_url: String,
    /// Scopes requested in addition to `openid`.
    pub scopes: Vec<String>,
    /// Provider name on the sign-in page.
    pub label: String,
}

impl OidcConfig {
    /// Reads `SOLIDATE_OIDC_ISSUER`, `SOLIDATE_OIDC_CLIENT_ID`,
    /// `SOLIDATE_OIDC_CLIENT_SECRET`, `SOLIDATE_OIDC_REDIRECT_URL` (default:
    /// `public_url` + [`CALLBACK_PATH`]), `SOLIDATE_OIDC_SCOPES` (space-separated,
    /// default `email profile`) and `SOLIDATE_OIDC_LABEL` (default `SSO`).
    /// `None` when `SOLIDATE_OIDC_ISSUER` is unset.
    pub fn from_env(public_url: Option<&str>) -> Result<Option<Self>, String> {
        fn var(name: &str) -> Option<String> {
            std::env::var(name)
                .ok()
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        }
        let Some(issuer) = var("SOLIDATE_OIDC_ISSUER") else {
            return Ok(None);
        };
        let client_id = var("SOLIDATE_OIDC_CLIENT_ID").ok_or("SOLIDATE_OIDC_CLIENT_ID must be set")?;
        let redirect_url = var("SOLIDATE_OIDC_REDIRECT_URL")
            .or_else(|| public_url.map(|u| format!("{}{CALLBACK_PATH}", u.trim_end_matches('/'))))
            .ok_or("SOLIDATE_OIDC_REDIRECT_URL or SOLIDATE_PUBLIC_URL must be set")?;
        let scopes = var("SOLIDATE_OIDC_SCOPES").unwrap_or_else(|| "email profile".to_owned());
        Ok(Some(Self {
            issuer,
            client_id,
            client_secret: var("SOLIDATE_OIDC_CLIENT_SECRET"),
            redirect_url,
            scopes: scopes
                .split_whitespace()
                .filter(|s| *s != "openid")
                .map(str::to_owned)
                .collect(),
            label: var("SOLIDATE_OIDC_LABEL").unwrap_or_else(|| "SSO".to_owned()),
        }))
    }
}

/// Per-attempt secrets kept by the front end between [`Oidc::begin`] and
/// [`Oidc::finish`]. All fields are base64url strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingLogin {
    pub state: String,
    pub nonce: String,
    pub pkce_verifier: String,
}

#[derive(Debug, Clone)]
pub struct OidcStart {
    /// Provider authorization URL to redirect the browser to.
    pub url: String,
    pub pending: PendingLogin,
}

/// Identity asserted by a verified ID token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OidcIdentity {
    pub issuer: String,
    pub subject: String,
    pub email: Option<String>,
    pub email_verified: bool,
    pub name: Option<String>,
}

/// A configured provider. Holds its discovery document and signing keys.
pub struct Oidc {
    config: OidcConfig,
    http: reqwest::Client,
    metadata: RwLock<CoreProviderMetadata>,
}

impl std::fmt::Debug for Oidc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Oidc")
            .field("issuer", &self.config.issuer)
            .finish_non_exhaustive()
    }
}

fn internal(context: &str, e: impl std::fmt::Display) -> AppError {
    AppError::Internal(format!("oidc {context}: {e}"))
}

fn rejected(reason: impl std::fmt::Display) -> AppError {
    tracing::warn!(target: "solidate::auth", %reason, "oidc login rejected");
    AppError::Unauthorized
}

impl Oidc {
    /// Fetches the provider's discovery document and signing keys.
    pub async fn discover(config: OidcConfig) -> Result<Self> {
        let http = reqwest::Client::builder()
            // Following redirects exposes the client to SSRF.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| internal("http client", e))?;
        let metadata = fetch_metadata(&config.issuer, &http).await?;
        Ok(Self::from_metadata(config, http, metadata))
    }

    /// Provider with a known discovery document, without network access.
    pub fn from_metadata(config: OidcConfig, http: reqwest::Client, metadata: CoreProviderMetadata) -> Self {
        Self {
            config,
            http,
            metadata: RwLock::new(metadata),
        }
    }

    pub fn label(&self) -> &str {
        &self.config.label
    }

    fn client(&self) -> Result<Client> {
        let metadata = self.metadata.read().expect("oidc metadata lock").clone();
        let redirect = RedirectUrl::new(self.config.redirect_url.clone()).map_err(|e| internal("redirect url", e))?;
        Ok(CoreClient::from_provider_metadata(
            metadata,
            ClientId::new(self.config.client_id.clone()),
            self.config.client_secret.clone().map(ClientSecret::new),
        )
        .set_redirect_uri(redirect))
    }

    /// Starts a sign-in attempt.
    pub fn begin(&self) -> Result<OidcStart> {
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let client = self.client()?;
        let mut request = client.authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        );
        for scope in &self.config.scopes {
            request = request.add_scope(Scope::new(scope.clone()));
        }
        let (url, state, nonce) = request.set_pkce_challenge(challenge).url();
        Ok(OidcStart {
            url: url.into(),
            pending: PendingLogin {
                state: state.into_secret(),
                nonce: nonce.secret().clone(),
                pkce_verifier: verifier.into_secret(),
            },
        })
    }

    /// Completes a sign-in attempt from the callback's `code` and `state`.
    /// `Unauthorized` when the state does not match or the provider's response
    /// fails verification.
    pub async fn finish(&self, pending: &PendingLogin, code: &str, state: &str) -> Result<OidcIdentity> {
        if !bool::from(state.as_bytes().ct_eq(pending.state.as_bytes())) {
            return Err(rejected("state mismatch"));
        }
        let client = self.client()?;
        let response = client
            .exchange_code(AuthorizationCode::new(code.to_owned()))
            .map_err(|e| internal("token endpoint", e))?
            .set_pkce_verifier(PkceCodeVerifier::new(pending.pkce_verifier.clone()))
            .request_async(&self.http)
            .await
            .map_err(|e| rejected(format!("code exchange: {e}")))?;
        let id_token = response.id_token().ok_or_else(|| rejected("no id token"))?;
        let nonce = Nonce::new(pending.nonce.clone());

        // An unknown signing key means the provider may have rotated keys:
        // reload them once and verify again.
        let refreshed;
        let mut verifier = client.id_token_verifier();
        if let Err(ClaimsVerificationError::SignatureVerification(_)) = id_token.claims(&verifier, &nonce) {
            let metadata = fetch_metadata(&self.config.issuer, &self.http).await?;
            *self.metadata.write().expect("oidc metadata lock") = metadata;
            refreshed = self.client()?;
            verifier = refreshed.id_token_verifier();
        }
        let claims = id_token
            .claims(&verifier, &nonce)
            .map_err(|e| rejected(format!("id token: {e}")))?;

        if let Some(expected) = claims.access_token_hash() {
            let alg = id_token.signing_alg().map_err(|e| rejected(format!("id token: {e}")))?;
            let key = id_token
                .signing_key(&verifier)
                .map_err(|e| rejected(format!("id token: {e}")))?;
            let actual = AccessTokenHash::from_token(response.access_token(), alg, key)
                .map_err(|e| rejected(format!("access token hash: {e}")))?;
            if actual != *expected {
                return Err(rejected("access token hash mismatch"));
            }
        }

        let name = claims
            .name()
            .and_then(|n| n.get(None))
            .map(|n| n.as_str().to_owned())
            .or_else(|| claims.preferred_username().map(|n| n.as_str().to_owned()));
        Ok(OidcIdentity {
            issuer: claims.issuer().as_str().to_owned(),
            subject: claims.subject().as_str().to_owned(),
            email: claims.email().map(|e| e.as_str().to_owned()),
            email_verified: claims.email_verified() == Some(true),
            name,
        })
    }
}

type Client =
    CoreClient<EndpointSet, EndpointNotSet, EndpointNotSet, EndpointNotSet, EndpointMaybeSet, EndpointMaybeSet>;

async fn fetch_metadata(issuer: &str, http: &reqwest::Client) -> Result<CoreProviderMetadata> {
    let issuer = IssuerUrl::new(issuer.to_owned()).map_err(|e| internal("issuer", e))?;
    CoreProviderMetadata::discover_async(issuer, http)
        .await
        .map_err(|e| internal("discovery", e))
}

impl App {
    pub fn oidc(&self) -> Option<&Oidc> {
        self.oidc.as_deref()
    }

    /// The user for a verified identity; see the module documentation for the
    /// mapping. `Invalid` when a new identity has no verified email,
    /// `Unauthorized` for disabled users.
    pub async fn oidc_login(&self, id: &OidcIdentity) -> Result<User> {
        let user = match self.db.identity_user(&id.issuer, &id.subject).await? {
            Some(u) => u,
            None => {
                let email = id
                    .email
                    .as_deref()
                    .map(str::trim)
                    .filter(|e| id.email_verified && e.contains('@') && e.len() <= 254)
                    .ok_or_else(|| invalid("the identity provider did not supply a verified email address"))?;
                match self.db.user_by_email(email).await? {
                    Some(u) => {
                        self.db.link_identity(&id.issuer, &id.subject, u.id).await?;
                        tracing::info!(target: "solidate::auth", user = %u.id, issuer = %id.issuer, "identity linked");
                        u
                    }
                    None => {
                        let name = id
                            .name
                            .as_deref()
                            .map(str::trim)
                            .filter(|n| !n.is_empty())
                            .unwrap_or(email);
                        let u = self
                            .db
                            .create_identity_user(email, name, &id.issuer, &id.subject)
                            .await?;
                        tracing::info!(target: "solidate::auth", user = %u.id, issuer = %id.issuer, "user created");
                        u
                    }
                }
            }
        };
        if user.disabled_at.is_some() {
            tracing::info!(target: "solidate::auth", user = %user.id, "login rejected: disabled");
            return Err(AppError::Unauthorized);
        }
        tracing::info!(target: "solidate::auth", user = %user.id, issuer = %id.issuer, "oidc login");
        Ok(user)
    }
}
