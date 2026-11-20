use serde_json::{Value, json};

use super::error::AuthError;
use super::http;

const USER_AUTH_URL: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_URL: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const CONTRACT_VERSION: &str = "1";

pub struct XboxToken {
    pub token: String,
    pub uhs: String,
}

pub async fn user_token(
    client: &reqwest::Client,
    ms_access_token: &str,
) -> Result<XboxToken, AuthError> {
    let body = json!({
        "Properties": {
            "AuthMethod": "RPS",
            "SiteName": "user.auth.xboxlive.com",
            "RpsTicket": format!("d={ms_access_token}"),
        },
        "RelyingParty": "http://auth.xboxlive.com",
        "TokenType": "JWT",
    });
    let value = post(client, USER_AUTH_URL, &body).await?;
    parse(value)
}

pub async fn xsts_token(
    client: &reqwest::Client,
    user_token: &str,
) -> Result<XboxToken, AuthError> {
    let body = json!({
        "Properties": {
            "SandboxId": "RETAIL",
            "UserTokens": [user_token],
        },
        "RelyingParty": "rp://api.minecraftservices.com/",
        "TokenType": "JWT",
    });
    let value = post(client, XSTS_URL, &body).await?;
    parse(value)
}

async fn post(client: &reqwest::Client, url: &str, body: &Value) -> Result<Value, AuthError> {
    let response = client
        .post(url)
        .header("x-xbl-contract-version", CONTRACT_VERSION)
        .json(body)
        .send()
        .await?;
    let status = response.status();
    let value = http::json_body(response).await?;
    if !status.is_success() {
        if let Some(code) = value.get("XErr").and_then(Value::as_i64) {
            return Err(map_xerr(code));
        }
        return Err(AuthError::Protocol(format!(
            "Xbox authentication failed (HTTP {status})"
        )));
    }
    Ok(value)
}

fn parse(value: Value) -> Result<XboxToken, AuthError> {
    let token = value
        .get("Token")
        .and_then(Value::as_str)
        .ok_or_else(|| AuthError::Protocol("Xbox authentication response is missing the token".to_owned()))?
        .to_owned();
    let uhs = value
        .pointer("/DisplayClaims/xui/0/uhs")
        .and_then(Value::as_str)
        .ok_or_else(|| AuthError::Protocol("Xbox authentication response is missing the user hash".to_owned()))?
        .to_owned();
    Ok(XboxToken { token, uhs })
}

fn map_xerr(code: i64) -> AuthError {
    let reason = match code {
        2148916227 => "this Xbox account is banned",
        2148916233 => "this Microsoft account has no Xbox account yet, create one at xbox.com",
        2148916235 => "Xbox Live is not available in this account's region",
        2148916236 | 2148916237 => "this account must complete adult verification first",
        2148916238 => "this is a child account and must be added to a Microsoft family by a parent",
        _ => return AuthError::Protocol(format!("Xbox authentication failed (XErr {code})")),
    };
    AuthError::Xbox(reason.to_owned())
}
