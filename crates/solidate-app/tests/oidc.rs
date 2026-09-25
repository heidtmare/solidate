//! OpenID Connect sign-in against an in-process provider. Require `DATABASE_URL`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{Duration, Utc};
use openidconnect::core::{
    CoreIdToken, CoreIdTokenClaims, CoreJsonWebKeySet, CoreJwsSigningAlgorithm, CoreProviderMetadata, CoreResponseType,
    CoreRsaPrivateSigningKey, CoreSubjectIdentifierType,
};
use openidconnect::url::Url;
use openidconnect::{
    AccessToken, Audience, AuthUrl, EmptyAdditionalClaims, EmptyAdditionalProviderMetadata, EndUserEmail, IssuerUrl,
    JsonWebKeyId, JsonWebKeySetUrl, Nonce, PkceCodeChallenge, PkceCodeVerifier, PrivateSigningKey, ResponseTypes,
    StandardClaims, SubjectIdentifier, TokenUrl,
};
use solidate_app::db::Db;
use solidate_app::{App, AppError, Config, Oidc, OidcConfig, OidcIdentity};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const CLIENT_ID: &str = "solidate";
const REDIRECT: &str = "https://docs.example/login/oidc/callback";

/// A code issued by the provider and the attempt it belongs to.
struct Issued {
    nonce: String,
    challenge: String,
    subject: String,
    email: Option<(String, bool)>,
}

struct State {
    issuer: String,
    key_pem: &'static str,
    kid: &'static str,
    codes: HashMap<String, Issued>,
}

#[derive(Clone)]
struct Provider(Arc<Mutex<State>>);

impl Provider {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let issuer = format!("http://{}", listener.local_addr().unwrap());
        let p = Self(Arc::new(Mutex::new(State {
            issuer,
            key_pem: include_str!("fixtures/oidc_key1.pem"),
            kid: "k1",
            codes: HashMap::new(),
        })));
        let server = p.clone();
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let server = server.clone();
                tokio::spawn(async move { server.serve(stream).await });
            }
        });
        p
    }

    fn issuer(&self) -> String {
        self.0.lock().unwrap().issuer.clone()
    }

    fn rotate_key(&self) {
        let mut s = self.0.lock().unwrap();
        s.key_pem = include_str!("fixtures/oidc_key2.pem");
        s.kid = "k2";
    }

    /// Simulates the user approving the attempt at `auth_url`; returns the code.
    fn approve(&self, auth_url: &str, subject: &str, email: Option<(&str, bool)>) -> String {
        let url = Url::parse(auth_url).unwrap();
        let q: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(q["client_id"], CLIENT_ID);
        assert_eq!(q["redirect_uri"], REDIRECT);
        assert_eq!(q["code_challenge_method"], "S256");
        assert!(q["scope"].split(' ').any(|s| s == "openid"));
        let code = format!("code-{}", q["state"]);
        self.0.lock().unwrap().codes.insert(
            code.clone(),
            Issued {
                nonce: q["nonce"].clone(),
                challenge: q["code_challenge"].clone(),
                subject: subject.to_owned(),
                email: email.map(|(e, v)| (e.to_owned(), v)),
            },
        );
        code
    }

    fn key(s: &State) -> CoreRsaPrivateSigningKey {
        CoreRsaPrivateSigningKey::from_pem(s.key_pem, Some(JsonWebKeyId::new(s.kid.into()))).unwrap()
    }

    fn respond(&self, method: &str, path: &str, body: &str) -> (u16, String) {
        let mut s = self.0.lock().unwrap();
        match (method, path) {
            ("GET", "/.well-known/openid-configuration") => {
                let metadata = CoreProviderMetadata::new(
                    IssuerUrl::new(s.issuer.clone()).unwrap(),
                    AuthUrl::new(format!("{}/authorize", s.issuer)).unwrap(),
                    JsonWebKeySetUrl::new(format!("{}/jwks", s.issuer)).unwrap(),
                    vec![ResponseTypes::new(vec![CoreResponseType::Code])],
                    vec![CoreSubjectIdentifierType::Public],
                    vec![CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256],
                    EmptyAdditionalProviderMetadata {},
                )
                .set_token_endpoint(Some(TokenUrl::new(format!("{}/token", s.issuer)).unwrap()));
                (200, serde_json::to_string(&metadata).unwrap())
            }
            ("GET", "/jwks") => {
                let jwks = CoreJsonWebKeySet::new(vec![Self::key(&s).as_verification_key()]);
                (200, serde_json::to_string(&jwks).unwrap())
            }
            ("POST", "/token") => {
                let form: HashMap<_, _> = openidconnect::url::form_urlencoded::parse(body.as_bytes())
                    .into_owned()
                    .collect();
                let invalid = (400, r#"{"error":"invalid_grant"}"#.to_owned());
                let Some(issued) = form.get("code").and_then(|c| s.codes.remove(c)) else {
                    return invalid;
                };
                let verifier = PkceCodeVerifier::new(form.get("code_verifier").cloned().unwrap_or_default());
                if PkceCodeChallenge::from_code_verifier_sha256(&verifier).as_str() != issued.challenge {
                    return invalid;
                }
                let access = AccessToken::new("access-token".into());
                let mut standard = StandardClaims::new(SubjectIdentifier::new(issued.subject));
                if let Some((email, verified)) = issued.email {
                    standard = standard
                        .set_email(Some(EndUserEmail::new(email)))
                        .set_email_verified(Some(verified));
                }
                let claims = CoreIdTokenClaims::new(
                    IssuerUrl::new(s.issuer.clone()).unwrap(),
                    vec![Audience::new(CLIENT_ID.into())],
                    Utc::now() + Duration::minutes(5),
                    Utc::now(),
                    standard,
                    EmptyAdditionalClaims {},
                )
                .set_nonce(Some(Nonce::new(issued.nonce)));
                let id_token = CoreIdToken::new(
                    claims,
                    &Self::key(&s),
                    CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
                    Some(&access),
                    None,
                )
                .unwrap();
                let body = serde_json::json!({
                    "access_token": access.secret(),
                    "token_type": "Bearer",
                    "expires_in": 300,
                    "id_token": id_token.to_string(),
                });
                (200, body.to_string())
            }
            _ => (404, "{}".to_owned()),
        }
    }

    /// Minimal HTTP/1.1: one request per connection.
    async fn serve(&self, mut stream: tokio::net::TcpStream) {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        let (head_len, content_len) = loop {
            let n = stream.read(&mut chunk).await.unwrap();
            if n == 0 {
                return;
            }
            buf.extend_from_slice(&chunk[..n]);
            if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&buf[..i]).to_lowercase();
                let len = head
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .map_or(0, |v| v.trim().parse().unwrap());
                break (i + 4, len);
            }
        };
        while buf.len() < head_len + content_len {
            let n = stream.read(&mut chunk).await.unwrap();
            buf.extend_from_slice(&chunk[..n]);
        }
        let head = String::from_utf8_lossy(&buf[..head_len]).into_owned();
        let mut request_line = head.split_whitespace();
        let method = request_line.next().unwrap_or_default();
        let path = request_line.next().unwrap_or_default();
        let body = String::from_utf8_lossy(&buf[head_len..]).into_owned();
        let (status, body) = self.respond(method, path, &body);
        let response = format!(
            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    }
}

async fn setup(pool: PgPoolOptions, opts: PgConnectOptions) -> (App, Provider) {
    let provider = Provider::start().await;
    let oidc = Oidc::discover(OidcConfig {
        issuer: provider.issuer(),
        client_id: CLIENT_ID.into(),
        client_secret: Some("secret".into()),
        redirect_url: REDIRECT.into(),
        scopes: vec!["email".into()],
        label: "Test".into(),
    })
    .await
    .unwrap();
    let app = App::new(Db::connect_with(pool, opts).await.unwrap(), Config::default()).with_oidc(oidc);
    (app, provider)
}

async fn sign_in(
    app: &App,
    provider: &Provider,
    subject: &str,
    email: Option<(&str, bool)>,
) -> Result<OidcIdentity, AppError> {
    let oidc = app.oidc().unwrap();
    let start = oidc.begin()?;
    let code = provider.approve(&start.url, subject, email);
    oidc.finish(&start.pending, &code, &start.pending.state).await
}

#[sqlx::test(migrator = "solidate_app::db::MIGRATOR")]
async fn code_flow_provisions_and_links(pool: PgPoolOptions, opts: PgConnectOptions) {
    let (app, provider) = setup(pool, opts).await;

    let id = sign_in(&app, &provider, "sub-alice", Some(("alice@example.com", true)))
        .await
        .unwrap();
    assert_eq!(id.issuer, provider.issuer());
    assert_eq!(id.subject, "sub-alice");
    assert_eq!(id.email.as_deref(), Some("alice@example.com"));
    assert!(id.email_verified);

    // New identity with a verified email: user created without a password.
    let alice = app.oidc_login(&id).await.unwrap();
    assert_eq!(alice.email, "alice@example.com");
    assert_eq!(alice.name, "alice@example.com");
    assert!(matches!(
        app.login("alice@example.com", "anything").await,
        Err(AppError::Unauthorized)
    ));

    // Linked identity: same user, even if the provider's email changes.
    let again = sign_in(&app, &provider, "sub-alice", Some(("alice@new.example", false)))
        .await
        .unwrap();
    assert_eq!(app.oidc_login(&again).await.unwrap().id, alice.id);

    // Existing password user: linked by verified email.
    let bob = app.register_user("bob@example.com", "Bob", "password1").await.unwrap();
    let id = sign_in(&app, &provider, "sub-bob", Some(("BOB@example.com", true)))
        .await
        .unwrap();
    assert_eq!(app.oidc_login(&id).await.unwrap().id, bob.id);
    assert_eq!(app.login("bob@example.com", "password1").await.unwrap().id, bob.id);

    // New identity without a verified email: rejected, nothing created.
    let id = sign_in(&app, &provider, "sub-carol", Some(("carol@example.com", false)))
        .await
        .unwrap();
    assert!(matches!(app.oidc_login(&id).await, Err(AppError::Invalid(_))));
    let id = sign_in(&app, &provider, "sub-carol", None).await.unwrap();
    assert!(matches!(app.oidc_login(&id).await, Err(AppError::Invalid(_))));
    assert!(app.db().user_by_email("carol@example.com").await.unwrap().is_none());

    // An unverified email does not take over an existing account.
    let id = sign_in(&app, &provider, "sub-mallory", Some(("bob@example.com", false)))
        .await
        .unwrap();
    assert!(matches!(app.oidc_login(&id).await, Err(AppError::Invalid(_))));
}

#[sqlx::test(migrator = "solidate_app::db::MIGRATOR")]
async fn disabled_user_is_rejected(pool: PgPoolOptions, opts: PgConnectOptions) {
    let owner = sqlx::PgPool::connect_with(opts.clone()).await.unwrap();
    let (app, provider) = setup(pool, opts).await;
    let id = sign_in(&app, &provider, "sub-dave", Some(("dave@example.com", true)))
        .await
        .unwrap();
    let dave = app.oidc_login(&id).await.unwrap();
    sqlx::query("UPDATE users SET disabled_at = now() WHERE id = $1")
        .bind(dave.id)
        .execute(&owner)
        .await
        .unwrap();
    assert!(matches!(app.oidc_login(&id).await, Err(AppError::Unauthorized)));
}

#[sqlx::test(migrator = "solidate_app::db::MIGRATOR")]
async fn callback_is_verified(pool: PgPoolOptions, opts: PgConnectOptions) {
    let (app, provider) = setup(pool, opts).await;
    let oidc = app.oidc().unwrap();
    let email = Some(("eve@example.com", true));

    // State from another attempt.
    let start = oidc.begin().unwrap();
    let code = provider.approve(&start.url, "sub-eve", email);
    let other = oidc.begin().unwrap();
    assert!(matches!(
        oidc.finish(&start.pending, &code, &other.pending.state).await,
        Err(AppError::Unauthorized)
    ));

    // Wrong PKCE verifier: the provider refuses the code.
    let start = oidc.begin().unwrap();
    let code = provider.approve(&start.url, "sub-eve", email);
    let mut pending = start.pending.clone();
    pending.pkce_verifier = other.pending.pkce_verifier.clone();
    assert!(matches!(
        oidc.finish(&pending, &code, &pending.state).await,
        Err(AppError::Unauthorized)
    ));

    // ID token issued for another attempt's nonce.
    let start = oidc.begin().unwrap();
    let code = provider.approve(&other.url, "sub-eve", email);
    let mut pending = start.pending.clone();
    pending.pkce_verifier = other.pending.pkce_verifier.clone();
    assert!(matches!(
        oidc.finish(&pending, &code, &pending.state).await,
        Err(AppError::Unauthorized)
    ));

    // A code is single-use.
    let start = oidc.begin().unwrap();
    let code = provider.approve(&start.url, "sub-eve", email);
    oidc.finish(&start.pending, &code, &start.pending.state).await.unwrap();
    assert!(matches!(
        oidc.finish(&start.pending, &code, &start.pending.state).await,
        Err(AppError::Unauthorized)
    ));
}

#[sqlx::test(migrator = "solidate_app::db::MIGRATOR")]
async fn signing_key_rotation_reloads_keys(pool: PgPoolOptions, opts: PgConnectOptions) {
    let (app, provider) = setup(pool, opts).await;
    let email = Some(("frank@example.com", true));
    sign_in(&app, &provider, "sub-frank", email).await.unwrap();
    provider.rotate_key();
    let id = sign_in(&app, &provider, "sub-frank", email).await.unwrap();
    assert_eq!(id.subject, "sub-frank");
}
