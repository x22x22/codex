use crate::ConfigRequirementsToml;
use crate::ConfigRequirementsWithSources;
use crate::LoadedConfigLayers;
use crate::RequirementSource;
#[cfg(target_os = "macos")]
use crate::macos::load_managed_admin_requirements_toml;
use codex_protocol::config_types::SandboxMode;
use codex_protocol::protocol::AskForApproval;
use codex_utils_absolute_path::AbsolutePathBuf;
use serde::Deserialize;
use std::io;
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;

#[cfg(unix)]
pub const SYSTEM_CONFIG_TOML_FILE_UNIX: &str = "/etc/codex/config.toml";

#[cfg(windows)]
const DEFAULT_PROGRAM_DATA_DIR_WINDOWS: &str = r"C:\ProgramData";

pub async fn load_requirements_toml(
    config_requirements_toml: &mut ConfigRequirementsWithSources,
    requirements_toml_file: impl AsRef<Path>,
) -> io::Result<()> {
    let requirements_toml_file =
        AbsolutePathBuf::from_absolute_path(requirements_toml_file.as_ref())?;
    match tokio::fs::read_to_string(&requirements_toml_file).await {
        Ok(contents) => {
            let requirements_config: ConfigRequirementsToml =
                toml::from_str(&contents).map_err(|err| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "Error parsing requirements file {}: {err}",
                            requirements_toml_file.as_ref().display(),
                        ),
                    )
                })?;
            config_requirements_toml.merge_unset_fields(
                RequirementSource::SystemRequirementsToml {
                    file: requirements_toml_file.clone(),
                },
                requirements_config,
            );
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(io::Error::new(
                err.kind(),
                format!(
                    "Failed to read requirements file {}: {err}",
                    requirements_toml_file.as_ref().display(),
                ),
            ));
        }
    }

    Ok(())
}

pub async fn load_managed_admin_requirements(
    config_requirements_toml: &mut ConfigRequirementsWithSources,
    managed_config_requirements_base64: Option<&str>,
) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        load_managed_admin_requirements_toml(
            config_requirements_toml,
            managed_config_requirements_base64,
        )
        .await
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = config_requirements_toml;
        let _ = managed_config_requirements_base64;
        Ok(())
    }
}

#[cfg(unix)]
pub fn system_requirements_toml_file() -> io::Result<AbsolutePathBuf> {
    AbsolutePathBuf::from_absolute_path(Path::new("/etc/codex/requirements.toml"))
}

#[cfg(windows)]
pub fn system_requirements_toml_file() -> io::Result<AbsolutePathBuf> {
    windows_system_requirements_toml_file()
}

#[cfg(unix)]
pub fn system_config_toml_file() -> io::Result<AbsolutePathBuf> {
    AbsolutePathBuf::from_absolute_path(Path::new(SYSTEM_CONFIG_TOML_FILE_UNIX))
}

#[cfg(windows)]
pub fn system_config_toml_file() -> io::Result<AbsolutePathBuf> {
    windows_system_config_toml_file()
}

#[cfg(windows)]
fn windows_codex_system_dir() -> PathBuf {
    let program_data = windows_program_data_dir_from_known_folder().unwrap_or_else(|err| {
        tracing::warn!(
            error = %err,
            "Failed to resolve ProgramData known folder; using default path"
        );
        PathBuf::from(DEFAULT_PROGRAM_DATA_DIR_WINDOWS)
    });
    program_data.join("OpenAI").join("Codex")
}

#[cfg(windows)]
fn windows_system_requirements_toml_file() -> io::Result<AbsolutePathBuf> {
    let requirements_toml_file = windows_codex_system_dir().join("requirements.toml");
    AbsolutePathBuf::try_from(requirements_toml_file)
}

#[cfg(windows)]
fn windows_system_config_toml_file() -> io::Result<AbsolutePathBuf> {
    let config_toml_file = windows_codex_system_dir().join("config.toml");
    AbsolutePathBuf::try_from(config_toml_file)
}

#[cfg(windows)]
fn windows_program_data_dir_from_known_folder() -> io::Result<PathBuf> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::FOLDERID_ProgramData;
    use windows_sys::Win32::UI::Shell::KF_FLAG_DEFAULT;
    use windows_sys::Win32::UI::Shell::SHGetKnownFolderPath;

    let mut path_ptr = std::ptr::null_mut::<u16>();
    let known_folder_flags = u32::try_from(KF_FLAG_DEFAULT).map_err(|_| {
        io::Error::other(format!(
            "KF_FLAG_DEFAULT did not fit in u32: {KF_FLAG_DEFAULT}"
        ))
    })?;
    let hr = unsafe {
        SHGetKnownFolderPath(&FOLDERID_ProgramData, known_folder_flags, 0, &mut path_ptr)
    };
    if hr != 0 {
        return Err(io::Error::other(format!(
            "SHGetKnownFolderPath(FOLDERID_ProgramData) failed with HRESULT {hr:#010x}"
        )));
    }
    if path_ptr.is_null() {
        return Err(io::Error::other(
            "SHGetKnownFolderPath(FOLDERID_ProgramData) returned a null pointer",
        ));
    }

    let path = unsafe {
        let mut len = 0usize;
        while *path_ptr.add(len) != 0 {
            len += 1;
        }
        let wide = std::slice::from_raw_parts(path_ptr, len);
        let path = PathBuf::from(OsString::from_wide(wide));
        CoTaskMemFree(path_ptr.cast());
        path
    };

    Ok(path)
}

pub async fn load_requirements_from_legacy_scheme(
    config_requirements_toml: &mut ConfigRequirementsWithSources,
    loaded_config_layers: LoadedConfigLayers,
) -> io::Result<()> {
    let LoadedConfigLayers {
        managed_config,
        managed_config_from_mdm,
    } = loaded_config_layers;

    for (source, config) in managed_config_from_mdm
        .map(|config| {
            (
                RequirementSource::LegacyManagedConfigTomlFromMdm,
                config.managed_config,
            )
        })
        .into_iter()
        .chain(managed_config.map(|config| {
            (
                RequirementSource::LegacyManagedConfigTomlFromFile { file: config.file },
                config.managed_config,
            )
        }))
    {
        let legacy_config: LegacyManagedConfigToml =
            config.try_into().map_err(|err: toml::de::Error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Failed to parse config requirements as TOML: {err}"),
                )
            })?;

        let requirements = ConfigRequirementsToml::from(legacy_config);
        config_requirements_toml.merge_unset_fields(source, requirements);
    }

    Ok(())
}

#[derive(Deserialize, Debug, Clone, Default, PartialEq)]
struct LegacyManagedConfigToml {
    approval_policy: Option<AskForApproval>,
    sandbox_mode: Option<SandboxMode>,
}

impl From<LegacyManagedConfigToml> for ConfigRequirementsToml {
    fn from(legacy: LegacyManagedConfigToml) -> Self {
        let mut config_requirements_toml = ConfigRequirementsToml::default();

        let LegacyManagedConfigToml {
            approval_policy,
            sandbox_mode,
        } = legacy;
        if let Some(approval_policy) = approval_policy {
            config_requirements_toml.allowed_approval_policies = Some(vec![approval_policy]);
        }
        if let Some(sandbox_mode) = sandbox_mode {
            let required_mode = sandbox_mode.into();
            let mut allowed_modes = vec![crate::SandboxModeRequirement::ReadOnly];
            if required_mode != crate::SandboxModeRequirement::ReadOnly {
                allowed_modes.push(required_mode);
            }
            config_requirements_toml.allowed_sandbox_modes = Some(allowed_modes);
        }
        config_requirements_toml
    }
}
