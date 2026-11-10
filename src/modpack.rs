pub mod archive;
pub mod curse;
pub mod mcbbs;
pub mod modrinth;
pub mod server;

use std::fs;
use std::path::Path;

use crate::download::{self, FileTask, Origins};
use crate::instance;
use crate::loader;
use crate::modpack::curse::CurseClient;
use crate::modpack::modrinth as modrinth_pack;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Modrinth,
    CurseForge,
    Mcbbs,
    Server,
    MultiMc,
}

impl Provider {
    pub fn name(self) -> &'static str {
        match self {
            Provider::Modrinth => "modrinth",
            Provider::CurseForge => "curseforge",
            Provider::Mcbbs => "mcbbs",
            Provider::Server => "server",
            Provider::MultiMc => "multimc",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoaderKind {
    Fabric,
    Forge,
    NeoForge,
}

impl LoaderKind {
    pub fn name(self) -> &'static str {
        match self {
            LoaderKind::Fabric => "fabric",
            LoaderKind::Forge => "forge",
            LoaderKind::NeoForge => "neoforge",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoaderSpec {
    pub kind: LoaderKind,
    pub version: Option<String>,
}

#[derive(Debug)]
pub struct PackFile {
    pub path: String,
    pub size: Option<u64>,
    pub sha1: Option<String>,
    pub sha512: Option<String>,
    pub urls: Vec<String>,
    pub optional: bool,
    pub client_unsupported: bool,
}

#[derive(Debug)]
pub struct PackManifest {
    pub provider: Provider,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub game_version: String,
    pub loader: Option<LoaderSpec>,
    pub files: Vec<PackFile>,
    pub overrides: Vec<String>,
    pub libraries: Vec<serde_json::Value>,
    pub manifest_name: String,
}

pub struct ImportOptions<'a> {
    pub mc_dir: &'a str,
    pub name: Option<&'a str>,
    pub java: Option<&'a str>,
    pub origins: Origins<'a>,
    pub with_optional: bool,
    pub dry_run: bool,
    pub source: &'a str,
    pub loader: Option<LoaderKind>,
    pub loader_version: Option<&'a str>,
}

pub struct ImportReport {
    pub id: String,
    pub provider: Provider,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub game_version: String,
    pub loader: Option<LoaderSpec>,
    pub files: usize,
    pub overrides: usize,
    pub excluded: Vec<String>,
    pub bytes: u64,
    pub installed: bool,
}

pub fn import_file(path: &Path, options: &ImportOptions<'_>) -> Result<ImportReport, String> {
    let detected = archive::detect(path)?;
    let mut manifest = read_manifest(path, &detected, options.with_optional)?;
    manifest.manifest_name = detected.manifest_name.clone();
    if let Some(kind) = options.loader {
        manifest.loader = Some(LoaderSpec {
            kind,
            version: options.loader_version.map(str::to_string),
        });
    } else if let Some(version) = options.loader_version {
        let loader = manifest.loader.as_mut().ok_or_else(|| {
            String::from("--loader-version was provided, but the modpack has no mod loader")
        })?;
        loader.version = Some(version.to_string());
    }

    if manifest.game_version.trim().is_empty() {
        return Err(String::from("modpack has no Minecraft version"));
    }

    let id = loader::instance_name(
        options.mc_dir,
        options.name,
        &format!("{}_{}", manifest.name, manifest.version),
    )?;
    let game_dir = Path::new(options.mc_dir).join("versions").join(&id);
    let java = crate::launch::resolve_java(options.java);

    let mut report = ImportReport {
        id: id.clone(),
        provider: manifest.provider,
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        author: manifest.author.clone(),
        description: manifest.description.clone(),
        game_version: manifest.game_version.clone(),
        loader: manifest.loader.clone(),
        files: 0,
        overrides: 0,
        excluded: Vec::new(),
        bytes: 0,
        installed: false,
    };

    if options.dry_run {
        report.files = manifest
            .files
            .iter()
            .filter(|file| !file.client_unsupported)
            .filter(|file| !file.optional || options.with_optional)
            .count();
        report.bytes = manifest
            .files
            .iter()
            .filter(|file| !file.client_unsupported)
            .filter(|file| !file.optional || options.with_optional)
            .filter_map(|file| file.size)
            .sum();
        report.excluded = manifest
            .files
            .iter()
            .filter(|file| file.optional && !options.with_optional)
            .map(|file| file.path.clone())
            .collect();
        return Ok(report);
    }

    let result = (|| -> Result<(), String> {
        install_loader(
            options.mc_dir,
            &manifest.game_version,
            manifest.loader.as_ref(),
            &id,
            &java,
            &options.origins,
        )?;

        append_libraries(options.mc_dir, &id, &manifest.libraries, &options.origins)?;

        fs::create_dir_all(&game_dir).map_err(|e| {
            format!(
                "failed to create instance directory {}: {e}",
                game_dir.display()
            )
        })?;

        report.overrides =
            archive::extract_overrides(path, &detected.prefix, &manifest.overrides, &game_dir)?;

        let mut tasks = Vec::new();
        let mut excluded = Vec::new();
        let mut paths = std::collections::HashSet::new();
        for file in &manifest.files {
            if file.client_unsupported {
                continue;
            }
            if file.optional && !options.with_optional {
                excluded.push(file.path.clone());
                continue;
            }
            if !paths.insert(file.path.clone()) {
                return Err(format!(
                    "modpack contains duplicate file path: {}",
                    file.path
                ));
            }

            let relative = archive::safe_relative(&file.path)?;
            report.bytes += file.size.unwrap_or(0);
            tasks.push(FileTask {
                urls: file.urls.clone(),
                dest: game_dir.join(relative),
                size: file.size,
                sha1: file.sha1.clone(),
                sha512: file.sha512.clone(),
            });
        }

        report.files = tasks.len();
        report.excluded = excluded.clone();
        download::download_all(tasks, &options.origins)?;

        if !archive::copy_entry(
            path,
            &detected.prefix,
            &manifest.manifest_name,
            &game_dir.join(&manifest.manifest_name),
        )? {
            return Err(format!(
                "modpack has no {} to preserve",
                manifest.manifest_name
            ));
        }

        instance::write_metadata(
            options.mc_dir,
            &id,
            true,
            Some(&instance::ModpackRecord {
                provider: manifest.provider.name(),
                name: &manifest.name,
                version: &manifest.version,
                source: options.source,
                manifest: &manifest.manifest_name,
                excluded: &excluded,
            }),
        )?;

        Ok(())
    })();

    if let Err(error) = result {
        let _ = fs::remove_dir_all(&game_dir);
        return Err(error);
    }

    report.installed = true;
    Ok(report)
}

fn read_manifest(
    path: &Path,
    detected: &archive::Detected,
    with_optional: bool,
) -> Result<PackManifest, String> {
    match detected.provider {
        Provider::Modrinth => {
            let text = read_manifest_text(path, &detected.prefix, "modrinth.index.json")?;
            let json: serde_json::Value = serde_json::from_str(&text)
                .map_err(|e| format!("failed to parse modrinth.index.json: {e}"))?;
            modrinth_pack::parse(&json)
        }
        Provider::CurseForge => {
            let text = read_manifest_text(path, &detected.prefix, "manifest.json")?;
            let json: serde_json::Value = serde_json::from_str(&text)
                .map_err(|e| format!("failed to parse CurseForge manifest.json: {e}"))?;
            let mut client = CurseClient::new();
            curse::parse(&json, &mut client, with_optional)
        }
        Provider::Mcbbs => {
            let text = read_manifest_text(path, &detected.prefix, "mcbbs.packmeta")
                .or_else(|_| read_manifest_text(path, &detected.prefix, "manifest.json"))?;
            let json: serde_json::Value = serde_json::from_str(&text)
                .map_err(|e| format!("failed to parse MCBBS manifest: {e}"))?;
            let mut client = CurseClient::new();
            mcbbs::parse(&json, &mut client)
        }
        Provider::Server => {
            let text = read_manifest_text(path, &detected.prefix, "server-manifest.json")?;
            let json: serde_json::Value = serde_json::from_str(&text)
                .map_err(|e| format!("failed to parse server-manifest.json: {e}"))?;
            server::parse(&json)
        }
        Provider::MultiMc => Err(String::from(
            "MultiMC modpack import is planned for the next batch and is not supported yet",
        )),
    }
}

fn read_manifest_text(path: &Path, prefix: &str, name: &str) -> Result<String, String> {
    let file =
        fs::File::open(path).map_err(|e| format!("failed to open {}: {e}", path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    archive::read_entry_text(&mut archive, &format!("{prefix}{name}"))?
        .ok_or_else(|| format!("modpack has no {name}"))
}

fn append_libraries(
    mc_dir: &str,
    id: &str,
    libraries: &[serde_json::Value],
    origins: &Origins<'_>,
) -> Result<(), String> {
    if libraries.is_empty() {
        return Ok(());
    }

    let path = Path::new(mc_dir)
        .join("versions")
        .join(id)
        .join(format!("{id}.json"));
    let text =
        fs::read_to_string(&path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    let mut version: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("failed to parse {}: {e}", path.display()))?;
    let entries = version["libraries"]
        .as_array_mut()
        .ok_or_else(|| String::from("version has no libraries array"))?;
    entries.extend(libraries.iter().cloned());

    let text = serde_json::to_string_pretty(&version).map_err(|e| e.to_string())?;
    fs::write(&path, text).map_err(|e| format!("failed to write {}: {e}", path.display()))?;
    download::ensure_libraries(mc_dir, &version, origins)
}

fn install_loader(
    mc_dir: &str,
    game_version: &str,
    loader: Option<&LoaderSpec>,
    id: &str,
    java: &str,
    origins: &Origins<'_>,
) -> Result<(), String> {
    match loader {
        None => download::download_instance(mc_dir, id, game_version, origins),
        Some(spec) => {
            let version = spec
                .version
                .as_deref()
                .ok_or_else(|| String::from("modpack does not specify a loader version"))?;
            match spec.kind {
                LoaderKind::Fabric => {
                    let base = download::ensure_version_json(mc_dir, game_version, origins)?;
                    loader::install_fabric_at(mc_dir, game_version, version, id, &base, origins)
                }
                LoaderKind::Forge => {
                    loader::install_forge_at(mc_dir, game_version, Some(version), id, java, origins)
                }
                LoaderKind::NeoForge => loader::install_neoforge_at(
                    mc_dir,
                    game_version,
                    Some(version),
                    id,
                    java,
                    origins,
                ),
            }
        }
    }
}
