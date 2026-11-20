use md5::{Digest, Md5};

use super::error::AuthError;
use super::model::Profile;

const MIN_NAME_LEN: usize = 3;
const MAX_NAME_LEN: usize = 16;

pub fn profile(name: &str) -> Result<Profile, AuthError> {
    if !is_valid_name(name) {
        return Err(AuthError::InvalidName(name.to_owned()));
    }
    Ok(Profile {
        id: offline_uuid(name),
        name: name.to_owned(),
    })
}

fn is_valid_name(name: &str) -> bool {
    let length = name.chars().count();
    (MIN_NAME_LEN..=MAX_NAME_LEN).contains(&length)
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

// 与原版服务端生成离线 UUID 的规则一致，否则联机时服务器和客户端算出的身份对不上
fn offline_uuid(name: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(format!("OfflinePlayer:{name}").as_bytes());
    let mut bytes = hasher.finalize();
    bytes[6] = (bytes[6] & 0x0f) | 0x30;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
