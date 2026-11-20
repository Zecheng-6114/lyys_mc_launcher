use serde_json::Value;

use super::error::AuthError;

pub async fn json_body(response: reqwest::Response) -> Result<Value, AuthError> {
    let status = response.status();
    let text = response.text().await?;
    serde_json::from_str(&text)
        .map_err(|_| AuthError::Protocol(format!("the service returned an unparseable response (HTTP {status})")))
}

pub fn describe_error(value: &Value, status: reqwest::StatusCode) -> String {
    let error = value.get("error").and_then(Value::as_str).unwrap_or("unknown error");
    let description = value
        .get("error_description")
        .and_then(Value::as_str)
        .unwrap_or("no description provided");
    format!("{error}: {description} (HTTP {status})")
}
