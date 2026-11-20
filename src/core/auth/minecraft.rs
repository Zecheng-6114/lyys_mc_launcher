use serde_json::{Value, json};

use super::error::AuthError;
use super::http;
use super::model::Profile;

const LOGIN_URL: &str = "https://api.minecraftservices.com/authentication/login_with_xbox";
const ENTITLEMENTS_URL: &str = "https://api.minecraftservices.com/entitlements/mcstore";
const PROFILE_URL: &str = "https://api.minecraftservices.com/minecraft/profile";

pub struct MinecraftToken {
    pub access_token: String,
    pub expires_in: u64,
}

pub struct Credentials {
    pub access_token: String,
    pub expires_in: u64,
    pub profile: Profile,
}

pub async fn authenticate(
    client: &reqwest::Client,
    xsts_token: &str,
    uhs: &str,
) -> Result<Credentials, AuthError> {
    let token = login(client, xsts_token, uhs).await?;
    check_ownership(client, &token.access_token).await?;
    let profile = fetch_profile(client, &token.access_token).await?;
    Ok(Credentials {
        access_token: token.access_token,
        expires_in: token.expires_in,
        profile,
    })
}

pub async fn login(
    client: &reqwest::Client,
    xsts_token: &str,
    uhs: &str,
) -> Result<MinecraftToken, AuthError> {
    let body = json!({ "identityToken": format!("XBL3.0 x={uhs};{xsts_token}") });
    let response = client.post(LOGIN_URL).json(&body).send().await?;
    let status = response.status();
    let value = http::json_body(response).await?;
    if !status.is_success() {
        let detail = value
            .get("errorMessage")
            .and_then(Value::as_str)
            .or_else(|| value.get("error").and_then(Value::as_str))
            .unwrap_or("no details provided");
        return Err(AuthError::Protocol(format!(
            "the Minecraft service rejected the login: {detail} (HTTP {status})"
        )));
    }
    let access_token = value
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| AuthError::Protocol("the Minecraft service did not return a game token".to_owned()))?
        .to_owned();
    let expires_in = value.get("expires_in").and_then(Value::as_u64).unwrap_or(86_400);
    Ok(MinecraftToken {
        access_token,
        expires_in,
    })
}

pub async fn check_ownership(
    client: &reqwest::Client,
    access_token: &str,
) -> Result<(), AuthError> {
    let response = client
        .get(ENTITLEMENTS_URL)
        .bearer_auth(access_token)
        .send()
        .await?;
    let status = response.status();
    let value = http::json_body(response).await?;
    if !status.is_success() {
        return Err(AuthError::Protocol(format!(
            "cannot verify account ownership (HTTP {status})"
        )));
    }
    let owned = value
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items.iter().any(|item| {
                matches!(
                    item.get("name").and_then(Value::as_str),
                    Some("product_minecraft") | Some("game_minecraft")
                )
            })
        })
        .unwrap_or(false);
    if owned {
        Ok(())
    } else {
        Err(AuthError::NoOwnership)
    }
}

pub async fn fetch_profile(
    client: &reqwest::Client,
    access_token: &str,
) -> Result<Profile, AuthError> {
    let response = client
        .get(PROFILE_URL)
        .bearer_auth(access_token)
        .send()
        .await?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(AuthError::NoProfile);
    }
    let status = response.status();
    let value = http::json_body(response).await?;
    if !status.is_success() {
        return Err(AuthError::Protocol(format!(
            "cannot fetch the Minecraft profile (HTTP {status})"
        )));
    }
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| AuthError::Protocol("the Minecraft profile is missing the UUID".to_owned()))?
        .to_owned();
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| AuthError::Protocol("the Minecraft profile is missing the name".to_owned()))?
        .to_owned();
    Ok(Profile { id, name })
}
