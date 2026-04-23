//! Google OAuth2 PKCE + loopback flow. Refresh token in keyring; access
//! tokens minted on demand.
//!
//! Replace `GOOGLE_CLIENT_ID` with your own OAuth 2.0 Desktop-app client ID
//! from <https://console.cloud.google.com/apis/credentials>. The loopback
//! redirect URI registered for that client should be `http://127.0.0.1`
//! (port is chosen at runtime by the plugin).

use std::sync::Mutex;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use keyring::Entry;
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::Manager;

const GOOGLE_CLIENT_ID: &str = "REPLACE_ME.apps.googleusercontent.com";
const SCOPES: &str = "openid email profile";
const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const KEYRING_SERVICE: &str = "evolvo.google";
const KEYRING_ACCOUNT: &str = "refresh_token";

#[derive(Default)]
pub struct OauthState {
    pending: Mutex<Option<PendingAuth>>,
}

struct PendingAuth {
    verifier: String,
    redirect_uri: String,
}

fn entry() -> Result<Entry, String> {
    Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT).map_err(|e| format!("keyring: {e}"))
}

fn gen_verifier() -> (String, String) {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill(&mut bytes);
    let verifier = URL_SAFE_NO_PAD.encode(bytes);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BeginResult {
    pub auth_url: String,
}

#[tauri::command]
pub async fn google_begin_sign_in(
    app: tauri::AppHandle,
    state: tauri::State<'_, OauthState>,
) -> Result<BeginResult, String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window missing".to_string())?;

    let port = tauri_plugin_oauth::start(move |url| {
        // The plugin emits the full redirect URL (with ?code=...) to the
        // frontend via a window event. The UI listens for `oauth://url` and
        // calls `google_complete_sign_in` with the code.
        let _ = window.emit("oauth://url", url);
    })
    .map_err(|e| format!("loopback start: {e}"))?;
    let redirect_uri = format!("http://127.0.0.1:{port}");

    let (verifier, challenge) = gen_verifier();
    {
        let mut guard = state.pending.lock().map_err(|e| e.to_string())?;
        *guard = Some(PendingAuth {
            verifier,
            redirect_uri: redirect_uri.clone(),
        });
    }

    let auth_url = url::Url::parse_with_params(
        AUTH_URL,
        &[
            ("client_id", GOOGLE_CLIENT_ID),
            ("redirect_uri", &redirect_uri),
            ("response_type", "code"),
            ("scope", SCOPES),
            ("access_type", "offline"),
            ("prompt", "consent"),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
        ],
    )
    .map_err(|e| e.to_string())?;

    Ok(BeginResult {
        auth_url: auth_url.to_string(),
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompleteArgs {
    pub code: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignInResult {
    pub access_token: String,
    pub expires_in: Option<u64>,
}

#[tauri::command]
pub async fn google_complete_sign_in(
    args: CompleteArgs,
    state: tauri::State<'_, OauthState>,
) -> Result<SignInResult, String> {
    let pending = {
        let mut guard = state.pending.lock().map_err(|e| e.to_string())?;
        guard.take().ok_or_else(|| "no pending auth".to_string())?
    };

    let res = reqwest::Client::new()
        .post(TOKEN_URL)
        .form(&[
            ("code", args.code.as_str()),
            ("client_id", GOOGLE_CLIENT_ID),
            ("redirect_uri", pending.redirect_uri.as_str()),
            ("grant_type", "authorization_code"),
            ("code_verifier", pending.verifier.as_str()),
        ])
        .send()
        .await
        .map_err(|e| format!("token exchange: {e}"))?;

    let parsed: TokenResponse = res.json().await.map_err(|e| format!("decode token: {e}"))?;
    if let Some(rt) = parsed.refresh_token.as_ref() {
        entry()?.set_password(rt).map_err(|e| format!("keyring set: {e}"))?;
    }
    Ok(SignInResult {
        access_token: parsed.access_token,
        expires_in: parsed.expires_in,
    })
}

#[tauri::command]
pub async fn google_is_signed_in() -> Result<bool, String> {
    Ok(entry()?.get_password().is_ok())
}

#[tauri::command]
pub async fn google_sign_out() -> Result<(), String> {
    let _ = entry()?.delete_credential();
    Ok(())
}

#[tauri::command]
pub async fn google_access_token() -> Result<String, String> {
    let refresh = entry()?
        .get_password()
        .map_err(|_| "user not signed in".to_string())?;
    let res = reqwest::Client::new()
        .post(TOKEN_URL)
        .form(&[
            ("refresh_token", refresh.as_str()),
            ("client_id", GOOGLE_CLIENT_ID),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await
        .map_err(|e| format!("refresh: {e}"))?;
    let parsed: TokenResponse = res.json().await.map_err(|e| format!("decode: {e}"))?;
    Ok(parsed.access_token)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInfo {
    pub sub: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub picture: Option<String>,
}

#[tauri::command]
pub async fn google_fetch_userinfo() -> Result<UserInfo, String> {
    let token = google_access_token().await?;
    let res = reqwest::Client::new()
        .get("https://openidconnect.googleapis.com/v1/userinfo")
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| format!("userinfo: {e}"))?;
    res.json().await.map_err(|e| format!("decode userinfo: {e}"))
}
