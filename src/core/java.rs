//! Java runtime discovery and version matching.
//!
//! The launcher does not bundle a runtime: when the user has not configured a
//! Java executable we scan the common installation locations and pick the one
//! whose major version matches the `javaVersion` requirement of the version
//! JSON.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use mc_launcher_core::utils::java::find_system_java_versions;

/// One Java runtime found on this machine.
#[derive(Clone, Debug)]
pub struct JavaRuntime {
    /// Executable used to start the game.
    pub executable: PathBuf,
    /// Major version, e.g. 8, 17 or 21.
    pub major: u32,
    /// Full version string reported by Java, e.g. `17.0.8`.
    pub version: String,
}

/// All Java runtimes found on this machine, newest major version first.
///
/// `configured` holds executables (or Java homes) remembered by the user; they
/// are probed in addition to the usual installation locations.
pub fn discover(minecraft_dir: &Path, configured: &[String]) -> Vec<JavaRuntime> {
    let mut runtimes = Vec::new();
    let mut seen = BTreeSet::new();

    for entry in configured {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let path = Path::new(entry);
        let executable = if path.is_dir() {
            path.join("bin").join(java_name())
        } else {
            path.to_path_buf()
        };
        if let Some(runtime) = probe_runtime(&executable) {
            let key = std::fs::canonicalize(&runtime.executable)
                .unwrap_or_else(|_| runtime.executable.clone());
            if seen.insert(key) {
                runtimes.push(runtime);
            }
        }
    }

    let mut homes: BTreeSet<PathBuf> = BTreeSet::new();
    if let Some(home) = std::env::var_os("JAVA_HOME").filter(|value| !value.is_empty()) {
        homes.insert(PathBuf::from(home));
    }
    homes.extend(path_java_homes());
    homes.extend(find_system_java_versions(None::<Vec<&str>>));
    homes.extend(bundled_runtime_homes(minecraft_dir));

    for home in homes {
        let Some(runtime) = probe_runtime(&home.join("bin").join(java_name())) else {
            continue;
        };
        let key = std::fs::canonicalize(&runtime.executable)
            .unwrap_or_else(|_| runtime.executable.clone());
        if seen.insert(key) {
            runtimes.push(runtime);
        }
    }
    runtimes.sort_by(|left, right| {
        right
            .major
            .cmp(&left.major)
            .then_with(|| version_key(&right.version).cmp(&version_key(&left.version)))
    });
    runtimes
}

/// Probe one Java executable and describe it.
fn probe_runtime(executable: &Path) -> Option<JavaRuntime> {
    // javaw 不会输出 `-version`，改为探测同目录的 java
    let windows = cfg!(windows);
    let probe_target = if windows
        && executable
            .file_name()
            .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("javaw.exe"))
    {
        executable.with_file_name("java.exe")
    } else {
        executable.to_path_buf()
    };
    if !probe_target.is_file() {
        return None;
    }
    let (major, version) = probe(&probe_target)?;
    // Windows 上优先用 javaw，启动游戏时不会附带一个控制台窗口
    let executable = if windows {
        let javaw = probe_target.with_file_name("javaw.exe");
        if javaw.is_file() { javaw } else { probe_target }
    } else {
        probe_target
    };
    Some(JavaRuntime {
        executable,
        major,
        version,
    })
}

/// Whether `actual` can run a version that requires `required`.
///
/// Modern versions tolerate newer releases (1.20.4 on Java 21 is fine), while
/// Java 8-era versions only work on Java 8 itself.
pub fn compatible(required: u32, actual: u32) -> bool {
    if required <= 8 {
        actual == 8
    } else {
        actual >= required
    }
}

/// Prefer an exact major match, otherwise the closest newer compatible runtime.
pub fn select(minecraft_dir: &Path, required: u32, configured: &[String]) -> Option<JavaRuntime> {
    let runtimes = discover(minecraft_dir, configured);
    if let Some(exact) = runtimes
        .iter()
        .find(|runtime| runtime.major == required)
    {
        return Some(exact.clone());
    }
    runtimes
        .into_iter()
        .filter(|runtime| compatible(required, runtime.major))
        .min_by_key(|runtime| runtime.major)
}

/// Major version of a Java executable, when it can be probed.
pub fn major_of(executable: &Path) -> Option<u32> {
    probe(executable).map(|(major, _)| major)
}

/// Run `<java> -version` and parse the quoted version from its output.
fn probe(executable: &Path) -> Option<(u32, String)> {
    let output = Command::new(executable).arg("-version").output().ok()?;
    let text = String::from_utf8_lossy(&output.stderr);
    for line in text.lines() {
        let Some(rest) = line.split("version \"").nth(1) else {
            continue;
        };
        let version = rest.split('"').next().unwrap_or_default();
        if let Some(major) = parse_major(version) {
            return Some((major, version.to_owned()));
        }
    }
    None
}

/// `1.8.0_392` -> 8, `17.0.8` -> 17, `25-ea` -> 25.
fn parse_major(version: &str) -> Option<u32> {
    let head: String = version
        .trim()
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == '.')
        .collect();
    let mut parts = head.split('.');
    let major = parts.next()?.parse::<u32>().ok()?;
    if major == 1 {
        parts.next()?.parse::<u32>().ok()
    } else {
        Some(major)
    }
}

fn version_key(version: &str) -> Vec<u32> {
    version
        .split(['.', '_', '-', '+'])
        .filter_map(|part| part.parse::<u32>().ok())
        .collect()
}

/// Java homes referenced by `java`/`java.exe` on `PATH`.
fn path_java_homes() -> Vec<PathBuf> {
    let file_name = if cfg!(windows) { "java.exe" } else { "java" };
    let Some(path) = std::env::var_os("PATH") else {
        return Vec::new();
    };
    std::env::split_paths(&path)
        .filter_map(|directory| {
            let executable = directory.join(file_name);
            if !executable.is_file() {
                return None;
            }
            // <home>/bin/java
            let bin = std::fs::canonicalize(&executable).ok()?;
            bin.parent()?.parent().map(Path::to_path_buf)
        })
        .collect()
}

/// Mojang runtimes installed under `<minecraft>/runtime`, e.g.
/// `runtime/java-runtime-gamma/linux/java-runtime-gamma/bin/java`.
fn bundled_runtime_homes(minecraft_dir: &Path) -> Vec<PathBuf> {
    let runtime_dir = minecraft_dir.join("runtime");
    let mut homes = Vec::new();
    let mut pending = vec![(runtime_dir, 0_usize)];
    while let Some((directory, depth)) = pending.pop() {
        if depth > 4 {
            continue;
        }
        if directory.join("bin").join(java_name()).is_file() {
            homes.push(directory);
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                pending.push((entry.path(), depth + 1));
            }
        }
    }
    homes
}

fn java_name() -> &'static str {
    if cfg!(windows) { "java.exe" } else { "java" }
}
