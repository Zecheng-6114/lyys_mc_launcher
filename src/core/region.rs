use std::time::Duration;

const IP_COUNTRY_URL: &str = "https://api.country.is/";
const CHINA_COUNTRY_CODE: &str = "CN";
const IP_TIMEOUT_SECONDS: u64 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    China,
    Other,
}

/// Detect the current region from the system time zone and the public IP.
///
/// A China time zone or a China IP is enough to treat the user as being in
/// China. `None` means neither source could be read, so the caller should
/// handle it as a non-China region and retry later.
pub async fn detect(client: &reqwest::Client) -> Option<Region> {
    let timezone = timezone_is_china();
    if timezone == Some(true) {
        return Some(Region::China);
    }
    let ip = ip_is_china(client).await;
    if ip == Some(true) {
        Some(Region::China)
    } else if timezone == Some(false) || ip == Some(false) {
        Some(Region::Other)
    } else {
        None
    }
}

fn timezone_is_china() -> Option<bool> {
    if let Some(tz) = std::env::var_os("TZ") {
        let tz = tz.to_string_lossy();
        let tz = tz.trim().trim_start_matches(':');
        if !tz.is_empty() {
            if tz.starts_with('/') {
                if let Ok(target) = std::fs::read_link(tz) {
                    return Some(is_china_timezone(&target.to_string_lossy()));
                }
            }
            return Some(is_china_timezone(tz));
        }
    }
    #[cfg(unix)]
    {
        if let Ok(tz) = std::fs::read_to_string("/etc/timezone") {
            let tz = tz.trim();
            if !tz.is_empty() {
                return Some(is_china_timezone(tz));
            }
        }
        if let Ok(target) = std::fs::read_link("/etc/localtime") {
            return Some(is_china_timezone(&target.to_string_lossy()));
        }
    }

    #[cfg(windows)]
    {
        windows_timezone().map(|tz| is_china_timezone(&tz))
    }

    #[cfg(not(windows))]
    {
        None
    }
}

#[cfg(windows)]
fn windows_timezone() -> Option<String> {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let output = std::process::Command::new("tzutil")
        .arg("/g")
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&output.stdout);
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

fn is_china_timezone(tz: &str) -> bool {
    let tz = tz.replace('\\', "/").to_ascii_lowercase();
    [
        "asia/shanghai",
        "asia/chongqing",
        "asia/harbin",
        "asia/urumqi",
        "asia/kashgar",
        "prc",
        "china",
        "中国",
        "中國",
    ]
    .iter()
    .copied()
    .any(|name| tz.contains(name))
}

async fn ip_is_china(client: &reqwest::Client) -> Option<bool> {
    let response = client
        .get(IP_COUNTRY_URL)
        .timeout(Duration::from_secs(IP_TIMEOUT_SECONDS))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let body: serde_json::Value = response.json().await.ok()?;
    let country = body.get("country")?.as_str()?;
    Some(country.eq_ignore_ascii_case(CHINA_COUNTRY_CODE))
}
