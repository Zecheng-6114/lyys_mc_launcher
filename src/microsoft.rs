use std::io::Read;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::download::encode_component;

const CLIENT_ID: &str = "f5caeb6d-1830-4c19-b809-83f41dcfd6bc";
const SCOPE: &str = "XboxLive.signin offline_access";
const DEVICE_CODE_URL: &str =
    "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode";
const TOKEN_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
const XBL_URL: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_URL: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const MC_LOGIN_URL: &str = "https://api.minecraftservices.com/authentication/login_with_xbox";
const MC_PROFILE_URL: &str = "https://api.minecraftservices.com/minecraft/profile";

const EXPIRY_MARGIN: u64 = 300;

pub struct MinecraftAccount {
    pub name: String,
    pub uuid: String,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: u64,
}

fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(20)))
            .timeout_global(Some(Duration::from_secs(120)))
            .http_status_as_error(false)
            .build()
            .new_agent()
    })
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

pub fn account_path() -> PathBuf {
    crate::loader::base_dir()
        .join("lyys_mc_launcher")
        .join("account.json")
}

pub fn load_account() -> Option<MinecraftAccount> {
    let text = std::fs::read_to_string(account_path()).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    Some(MinecraftAccount {
        name: value["name"].as_str()?.to_string(),
        uuid: value["uuid"].as_str()?.to_string(),
        access_token: value["access_token"].as_str()?.to_string(),
        refresh_token: value["refresh_token"].as_str()?.to_string(),
        expires_at: value["expires_at"].as_u64().unwrap_or(0),
    })
}

pub fn account_name() -> Option<String> {
    load_account().map(|account| account.name)
}

pub fn save_account(account: &MinecraftAccount) -> Result<(), String> {
    let path = account_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("failed to create {}: {e}", dir.display()))?;
    }

    let value = json!({
        "name": account.name,
        "uuid": account.uuid,
        "access_token": account.access_token,
        "refresh_token": account.refresh_token,
        "expires_at": account.expires_at,
    });
    let text = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;

    let temp = path.with_file_name(format!("account-{}.tmp", std::process::id()));
    let cleanup = |error: String| {
        let _ = std::fs::remove_file(&temp);
        error
    };
    std::fs::write(&temp, text)
        .map_err(|e| cleanup(format!("failed to write {}: {e}", temp.display())))?;
    std::fs::rename(&temp, &path)
        .map_err(|e| cleanup(format!("failed to write {}: {e}", path.display())))
}

pub fn logout() -> Result<(), String> {
    let path = account_path();
    if !path.exists() {
        return Ok(());
    }
    std::fs::remove_file(&path).map_err(|e| format!("failed to remove {}: {e}", path.display()))
}

pub fn login() -> Result<(), String> {
    let body = form(&[("client_id", CLIENT_ID), ("scope", SCOPE)]);
    let (status, text) = post_form(DEVICE_CODE_URL, &body)?;
    let response = parse_json(&text)?;
    if status != 200 {
        return Err(format!(
            "device code request failed (HTTP {status}): {}",
            response["error_description"].as_str().unwrap_or(&text)
        ));
    }

    let device_code = response["device_code"]
        .as_str()
        .ok_or_else(|| String::from("device code response missing device_code"))?
        .to_string();
    let user_code = response["user_code"].as_str().unwrap_or("").to_string();
    let uri = response["verification_uri"]
        .as_str()
        .unwrap_or("https://www.microsoft.com/link")
        .to_string();
    let interval = response["interval"].as_u64().unwrap_or(5).max(1);
    let expires_in = response["expires_in"].as_u64().unwrap_or(900);

    println!("open {uri} in a browser and sign in with code: {user_code}");

    let msa = wait_for_token(&device_code, interval, expires_in)?;
    let account = minecraft_session(&msa.0, &msa.1)?;
    save_account(&account)?;
    println!("logged in as {}", account.name);
    Ok(())
}

pub fn ensure_fresh(account: &mut MinecraftAccount) -> Result<(), String> {
    if now() + EXPIRY_MARGIN < account.expires_at {
        return Ok(());
    }

    let body = form(&[
        ("client_id", CLIENT_ID),
        ("grant_type", "refresh_token"),
        ("refresh_token", account.refresh_token.as_str()),
    ]);
    let (status, text) = post_form(TOKEN_URL, &body)?;
    let value = parse_json(&text)?;
    if status != 200 {
        let error = value["error"].as_str().unwrap_or("");
        if error == "invalid_grant" {
            return Err(String::from(
                "saved sign-in expired, please run `lyys_mc_launcher login` again",
            ));
        }
        return Err(format!(
            "token refresh failed (HTTP {status}): {}",
            value["error_description"].as_str().unwrap_or("unknown error")
        ));
    }

    let access = value["access_token"]
        .as_str()
        .ok_or_else(|| String::from("refresh response missing access_token"))?
        .to_string();
    let refresh = value["refresh_token"]
        .as_str()
        .unwrap_or(account.refresh_token.as_str())
        .to_string();

    let fresh = minecraft_session(&access, &refresh)?;
    *account = fresh;
    save_account(account)?;
    Ok(())
}

fn wait_for_token(
    device_code: &str,
    mut interval: u64,
    expires_in: u64,
) -> Result<(String, String), String> {
    let deadline = now() + expires_in;

    loop {
        std::thread::sleep(Duration::from_secs(interval));
        if now() > deadline {
            return Err(String::from("device code expired, please try again"));
        }

        let body = form(&[
            ("client_id", CLIENT_ID),
            ("device_code", device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ]);
        let (status, text) = post_form(TOKEN_URL, &body)?;
        let value = parse_json(&text)?;

        if status == 200 {
            let access = value["access_token"]
                .as_str()
                .ok_or_else(|| String::from("token response missing access_token"))?
                .to_string();
            let refresh = value["refresh_token"]
                .as_str()
                .ok_or_else(|| {
                    String::from("token response missing refresh_token (offline_access denied)")
                })?
                .to_string();
            return Ok((access, refresh));
        }

        let error = value["error"].as_str().unwrap_or("");
        match error {
            "authorization_pending" => continue,
            "slow_down" => {
                interval += 5;
                continue;
            }
            "expired_token" => return Err(String::from("device code expired, please try again")),
            "authorization_declined" => return Err(String::from("sign-in was declined")),
            _ => {
                return Err(format!(
                    "sign-in failed: {}",
                    value["error_description"].as_str().unwrap_or(error)
                ))
            }
        }
    }
}

fn minecraft_session(msa_token: &str, refresh_token: &str) -> Result<MinecraftAccount, String> {
    let xbl = xbl_token(msa_token)?;
    let (xsts, uhs) = xsts_token(&xbl)?;

    let body = json!({
        "identityToken": format!("XBL3.0 x={uhs};{xsts}"),
    });
    let (status, text) = post_json(MC_LOGIN_URL, &body)?;
    let value = parse_json(&text)?;
    if status != 200 {
        return Err(format!("Minecraft sign-in failed (HTTP {status})"));
    }
    let access = value["access_token"]
        .as_str()
        .ok_or_else(|| String::from("Minecraft sign-in response missing access_token"))?
        .to_string();
    let expires_in = value["expires_in"]
        .as_u64()
        .unwrap_or(86_400)
        .saturating_sub(EXPIRY_MARGIN);

    let (status, text) = get_bearer(MC_PROFILE_URL, &access)?;
    let profile = parse_json(&text)?;
    if status != 200 {
        return Err(format!(
            "failed to load Minecraft profile (HTTP {status}): this account has no game profile",
        ));
    }
    let name = profile["name"]
        .as_str()
        .ok_or_else(|| String::from("profile response missing name"))?
        .to_string();
    let uuid = dashed_uuid(profile["id"].as_str().unwrap_or(""))?;

    Ok(MinecraftAccount {
        name,
        uuid,
        access_token: access,
        refresh_token: refresh_token.to_string(),
        expires_at: now() + expires_in,
    })
}

fn xbl_token(msa_token: &str) -> Result<String, String> {
    let body = json!({
        "Properties": {
            "AuthMethod": "RPS",
            "SiteName": "user.auth.xboxlive.com",
            "RpsTicket": format!("d={msa_token}"),
        },
        "RelyingParty": "http://auth.xboxlive.com",
        "TokenType": "JWT",
    });
    let (status, text) = post_json(XBL_URL, &body)?;
    let value = parse_json(&text)?;
    if status != 200 {
        return Err(format!("Xbox Live sign-in failed (HTTP {status})"));
    }
    value["Token"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| String::from("Xbox Live response missing token"))
}

fn xsts_token(xbl: &str) -> Result<(String, String), String> {
    let body = json!({
        "Properties": {
            "SandboxId": "RETAIL",
            "UserTokens": [xbl],
        },
        "RelyingParty": "rp://api.minecraftservices.com/",
        "TokenType": "JWT",
    });
    let (status, text) = post_json(XSTS_URL, &body)?;
    let value = parse_json(&text)?;
    if status != 200 {
        let reason = match value["XErr"].as_u64() {
            Some(214_891_623_3) => "this Microsoft account has no Minecraft profile",
            Some(214_891_623_8) => "this is a child account and needs parental verification",
            Some(214_891_623_6) | Some(214_891_623_7) => {
                "this account needs additional verification"
            }
            _ => "Xbox Live denied the request",
        };
        return Err(format!(
            "Xbox Live XSTS sign-in failed (HTTP {status}): {reason}",
        ));
    }
    let token = value["Token"]
        .as_str()
        .ok_or_else(|| String::from("XSTS response missing token"))?
        .to_string();
    let uhs = value["DisplayClaims"]["xui"][0]["uhs"]
        .as_str()
        .ok_or_else(|| String::from("XSTS response missing user hash"))?
        .to_string();
    Ok((token, uhs))
}

fn dashed_uuid(hex: &str) -> Result<String, String> {
    let hex = hex.trim().to_ascii_lowercase();
    if hex.len() == 36 && hex.matches('-').count() == 4 {
        return Ok(hex);
    }
    if hex.len() != 32 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("profile id is not a uuid: {hex}"));
    }

    let bytes: Vec<char> = hex.chars().collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        bytes[0..8].iter().collect::<String>(),
        bytes[8..12].iter().collect::<String>(),
        bytes[12..16].iter().collect::<String>(),
        bytes[16..20].iter().collect::<String>(),
        bytes[20..32].iter().collect::<String>(),
    ))
}

fn form(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(name, value)| format!("{}={}", encode_component(name), encode_component(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn parse_json(text: &str) -> Result<Value, String> {
    serde_json::from_str(text).map_err(|e| format!("failed to parse auth response: {e}"))
}

fn post_form(url: &str, body: &str) -> Result<(u16, String), String> {
    let resp = agent()
        .post(url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .send(body.to_string())
        .map_err(|e| format!("request failed {url}: {e}"))?;
    finish(resp)
}

fn post_json(url: &str, body: &Value) -> Result<(u16, String), String> {
    let bytes = serde_json::to_vec(body).map_err(|e| e.to_string())?;
    let resp = agent()
        .post(url)
        .header("Content-Type", "application/json")
        .send(bytes.as_slice())
        .map_err(|e| format!("request failed {url}: {e}"))?;
    finish(resp)
}

fn get_bearer(url: &str, token: &str) -> Result<(u16, String), String> {
    let resp = agent()
        .get(url)
        .header("Authorization", &format!("Bearer {token}"))
        .call()
        .map_err(|e| format!("request failed {url}: {e}"))?;
    finish(resp)
}

fn finish(resp: ureq::http::Response<ureq::Body>) -> Result<(u16, String), String> {
    let status = resp.status().as_u16();
    let mut text = String::new();
    resp.into_body()
        .into_reader()
        .read_to_string(&mut text)
        .map_err(|e| format!("read failed: {e}"))?;
    Ok((status, text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dashed_uuid_formats_hex_and_keeps_dashed() {
        let hex = "d0efb1ac8e3a4f55a2bda34f9d24d0a5";
        assert_eq!(
            dashed_uuid(hex).unwrap(),
            "d0efb1ac-8e3a-4f55-a2bd-a34f9d24d0a5"
        );
        let dashed = "d0efb1ac-8e3a-4f55-a2bd-a34f9d24d0a5";
        assert_eq!(dashed_uuid(dashed).unwrap(), dashed);
        assert!(dashed_uuid("nothex").is_err());
    }

    #[test]
    fn form_pairs_are_urlencoded() {
        assert_eq!(
            form(&[("a", "x y"), ("grant_type", "device_code")]),
            "a=x%20y&grant_type=device_code"
        );
    }
}
