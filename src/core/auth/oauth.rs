use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::Value;

use super::error::AuthError;
use super::http;
use super::model::DeviceCodePrompt;

const DEVICE_CODE_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode";
const TOKEN_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
const SCOPE: &str = "XboxLive.signin offline_access";

pub struct MsTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
}

pub struct DeviceCodeRequest {
    pub prompt: DeviceCodePrompt,
    pub device_code: String,
    pub interval: u64,
    pub expires_in: u64,
}

#[derive(Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: u64,
}

pub async fn request_device_code(
    client: &reqwest::Client,
    client_id: &str,
) -> Result<DeviceCodeRequest, AuthError> {
    let response = client
        .post(DEVICE_CODE_URL)
        .form(&[("client_id", client_id), ("scope", SCOPE)])
        .send()
        .await?;
    let status = response.status();
    let value = http::json_body(response).await?;
    if !status.is_success() {
        return Err(AuthError::Protocol(http::describe_error(&value, status)));
    }
    let parsed: DeviceCodeResponse = serde_json::from_value(value)
        .map_err(|_| AuthError::Protocol("device code response is missing required fields".to_owned()))?;
    Ok(DeviceCodeRequest {
        prompt: DeviceCodePrompt {
            verification_uri: parsed.verification_uri,
            user_code: parsed.user_code,
            expires_in: parsed.expires_in,
        },
        device_code: parsed.device_code,
        interval: parsed.interval,
        expires_in: parsed.expires_in,
    })
}

pub async fn poll_device_token(
    client: &reqwest::Client,
    client_id: &str,
    request: &DeviceCodeRequest,
) -> Result<MsTokens, AuthError> {
    let deadline = Instant::now() + Duration::from_secs(request.expires_in);
    let mut interval = Duration::from_secs(request.interval.max(1));
    loop {
        tokio::time::sleep(interval).await;
        if Instant::now() >= deadline {
            return Err(AuthError::DeviceCodeExpired);
        }
        let response = client
            .post(TOKEN_URL)
            .form(&[
                ("client_id", client_id),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", request.device_code.as_str()),
            ])
            .send()
            .await?;
        let status = response.status();
        let value = http::json_body(response).await?;
        if status.is_success() {
            return tokens_from(&value);
        }
        match value.get("error").and_then(Value::as_str) {
            Some("authorization_pending") => continue,
            Some("slow_down") => {
                interval += Duration::from_secs(5);
                continue;
            }
            Some("expired_token") => return Err(AuthError::DeviceCodeExpired),
            Some("authorization_declined") | Some("access_denied") => {
                return Err(AuthError::Protocol("the authorization was declined".to_owned()));
            }
            _ => return Err(AuthError::Protocol(http::describe_error(&value, status))),
        }
    }
}

pub async fn refresh(
    client: &reqwest::Client,
    client_id: &str,
    refresh_token: &str,
) -> Result<MsTokens, AuthError> {
    let response = client
        .post(TOKEN_URL)
        .form(&[
            ("client_id", client_id),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("scope", SCOPE),
        ])
        .send()
        .await?;
    let status = response.status();
    let value = http::json_body(response).await?;
    if status.is_success() {
        return tokens_from(&value);
    }
    match value.get("error").and_then(Value::as_str) {
        Some("invalid_grant") | Some("interaction_required") => Err(AuthError::NeedsRelogin),
        _ => Err(AuthError::Protocol(http::describe_error(&value, status))),
    }
}

fn tokens_from(value: &Value) -> Result<MsTokens, AuthError> {
    let access_token = value
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| AuthError::Protocol("token response is missing access_token".to_owned()))?
        .to_owned();
    let refresh_token = value
        .get("refresh_token")
        .and_then(Value::as_str)
        .ok_or_else(|| AuthError::Protocol("token response is missing refresh_token".to_owned()))?
        .to_owned();
    let expires_in = value.get("expires_in").and_then(Value::as_u64).unwrap_or(3600);
    Ok(MsTokens {
        access_token,
        refresh_token,
        expires_in,
    })
}
