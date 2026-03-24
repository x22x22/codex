#[cfg(target_os = "macos")]
use super::EffectiveSandboxPermissions;
use super::SandboxManager;
use super::effective_file_system_sandbox_policy;
#[cfg(target_os = "macos")]
use super::intersect_permission_profiles;
use super::merge_file_system_policy_with_additional_permissions;
use super::normalize_additional_permissions;
use super::sandbox_policy_with_additional_permissions;
use super::should_require_platform_sandbox;
use crate::exec::ExecCapturePolicy;
use crate::exec::ExecExpiration;
use crate::exec::SandboxType;
use crate::protocol::NetworkAccess;
use crate::protocol::ReadOnlyAccess;
use crate::protocol::SandboxPolicy;
use crate::tools::sandboxing::SandboxablePreference;
use codex_protocol::config_types::WindowsSandboxLevel;
use codex_protocol::models::FileSystemPermissions;
#[cfg(target_os = "macos")]
use codex_protocol::models::MacOsAutomationPermission;
#[cfg(target_os = "macos")]
use codex_protocol::models::MacOsContactsPermission;
#[cfg(target_os = "macos")]
use codex_protocol::models::MacOsPreferencesPermission;
#[cfg(target_os = "macos")]
use codex_protocol::models::MacOsSeatbeltProfileExtensions;
use codex_protocol::models::NetworkPermissions;
use codex_protocol::models::PermissionProfile;
use codex_protocol::permissions::FileSystemAccessMode;
use codex_protocol::permissions::FileSystemPath;
use codex_protocol::permissions::FileSystemSandboxEntry;
use codex_protocol::permissions::FileSystemSandboxPolicy;
use codex_protocol::permissions::FileSystemSpecialPath;
use codex_protocol::permissions::NetworkSandboxPolicy;
use codex_utils_absolute_path::AbsolutePathBuf;
#[cfg(target_os = "linux")]
use core_test_support::codex_linux_sandbox_exe_or_skip;
#[cfg(unix)]
use core_test_support::skip_if_sandbox;
use dunce::canonicalize;
use pretty_assertions::assert_eq;
use std::collections::HashMap;
#[cfg(unix)]
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;
use tempfile::TempDir;

#[cfg(unix)]
fn symlink_dir(original: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(original, link)
}

#[cfg(unix)]
fn sandbox_command_spec(
    command: Vec<String>,
    cwd: PathBuf,
    env: HashMap<String, String>,
) -> super::CommandSpec {
    let mut args = command.into_iter();
    let program = args.next().expect("command should not be empty");
    super::CommandSpec {
        program,
        args: args.collect(),
        cwd,
        env,
        expiration: ExecExpiration::DefaultTimeout,
        capture_policy: ExecCapturePolicy::ShellTool,
        sandbox_permissions: super::SandboxPermissions::UseDefault,
        additional_permissions: None,
        justification: None,
    }
}

#[cfg(unix)]
async fn execute_command_under_platform_sandbox(
    command: Vec<String>,
    command_cwd: PathBuf,
    sandbox_policy: &SandboxPolicy,
    env: HashMap<String, String>,
    codex_linux_sandbox_exe: Option<&PathBuf>,
) -> crate::error::Result<crate::exec::ExecToolCallOutput> {
    let file_system_policy =
        FileSystemSandboxPolicy::from_legacy_sandbox_policy(sandbox_policy, command_cwd.as_path());
    let network_policy = NetworkSandboxPolicy::from(sandbox_policy);
    let manager = SandboxManager::new();
    let exec_request = manager
        .transform(super::SandboxTransformRequest {
            spec: sandbox_command_spec(command, command_cwd.clone(), env),
            policy: sandbox_policy,
            file_system_policy: &file_system_policy,
            network_policy,
            #[cfg(target_os = "macos")]
            sandbox: SandboxType::MacosSeatbelt,
            #[cfg(target_os = "linux")]
            sandbox: SandboxType::LinuxSeccomp,
            enforce_managed_network: false,
            network: None,
            sandbox_policy_cwd: command_cwd.as_path(),
            #[cfg(target_os = "macos")]
            macos_seatbelt_profile_extensions: None,
            codex_linux_sandbox_exe,
            use_legacy_landlock: false,
            windows_sandbox_level: WindowsSandboxLevel::Disabled,
            windows_sandbox_private_desktop: false,
        })
        .expect("transform");
    super::execute_env(exec_request, None).await
}

#[cfg(target_os = "linux")]
async fn linux_sandbox_is_enforceable(codex_linux_sandbox_exe: &PathBuf) -> bool {
    let command_cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(_) => return false,
    };
    execute_command_under_platform_sandbox(
        vec!["/usr/bin/true".to_string()],
        command_cwd,
        &SandboxPolicy::new_read_only_policy(),
        HashMap::new(),
        Some(codex_linux_sandbox_exe),
    )
    .await
    .map(|output| output.exit_code == 0)
    .unwrap_or(false)
}

#[test]
fn danger_full_access_defaults_to_no_sandbox_without_network_requirements() {
    let manager = SandboxManager::new();
    let sandbox = manager.select_initial(
        &FileSystemSandboxPolicy::unrestricted(),
        NetworkSandboxPolicy::Enabled,
        SandboxablePreference::Auto,
        WindowsSandboxLevel::Disabled,
        false,
    );
    assert_eq!(sandbox, SandboxType::None);
}

#[test]
fn danger_full_access_uses_platform_sandbox_with_network_requirements() {
    let manager = SandboxManager::new();
    let expected = crate::safety::get_platform_sandbox(false).unwrap_or(SandboxType::None);
    let sandbox = manager.select_initial(
        &FileSystemSandboxPolicy::unrestricted(),
        NetworkSandboxPolicy::Enabled,
        SandboxablePreference::Auto,
        WindowsSandboxLevel::Disabled,
        true,
    );
    assert_eq!(sandbox, expected);
}

#[test]
fn restricted_file_system_uses_platform_sandbox_without_managed_network() {
    let manager = SandboxManager::new();
    let expected = crate::safety::get_platform_sandbox(false).unwrap_or(SandboxType::None);
    let sandbox = manager.select_initial(
        &FileSystemSandboxPolicy::restricted(vec![FileSystemSandboxEntry {
            path: FileSystemPath::Special {
                value: FileSystemSpecialPath::Root,
            },
            access: FileSystemAccessMode::Read,
        }]),
        NetworkSandboxPolicy::Enabled,
        SandboxablePreference::Auto,
        WindowsSandboxLevel::Disabled,
        false,
    );
    assert_eq!(sandbox, expected);
}

#[test]
fn full_access_restricted_policy_skips_platform_sandbox_when_network_is_enabled() {
    let policy = FileSystemSandboxPolicy::restricted(vec![FileSystemSandboxEntry {
        path: FileSystemPath::Special {
            value: FileSystemSpecialPath::Root,
        },
        access: FileSystemAccessMode::Write,
    }]);

    assert_eq!(
        should_require_platform_sandbox(&policy, NetworkSandboxPolicy::Enabled, false),
        false
    );
}

#[test]
fn root_write_policy_with_carveouts_still_uses_platform_sandbox() {
    let blocked = AbsolutePathBuf::resolve_path_against_base(
        "blocked",
        std::env::current_dir().expect("current dir"),
    )
    .expect("blocked path");
    let policy = FileSystemSandboxPolicy::restricted(vec![
        FileSystemSandboxEntry {
            path: FileSystemPath::Special {
                value: FileSystemSpecialPath::Root,
            },
            access: FileSystemAccessMode::Write,
        },
        FileSystemSandboxEntry {
            path: FileSystemPath::Path { path: blocked },
            access: FileSystemAccessMode::None,
        },
    ]);

    assert_eq!(
        should_require_platform_sandbox(&policy, NetworkSandboxPolicy::Enabled, false),
        true
    );
}

#[test]
fn full_access_restricted_policy_still_uses_platform_sandbox_for_restricted_network() {
    let policy = FileSystemSandboxPolicy::restricted(vec![FileSystemSandboxEntry {
        path: FileSystemPath::Special {
            value: FileSystemSpecialPath::Root,
        },
        access: FileSystemAccessMode::Write,
    }]);

    assert_eq!(
        should_require_platform_sandbox(&policy, NetworkSandboxPolicy::Restricted, false),
        true
    );
}

#[test]
fn transform_preserves_unrestricted_file_system_policy_for_restricted_network() {
    let manager = SandboxManager::new();
    let cwd = std::env::current_dir().expect("current dir");
    let exec_request = manager
        .transform(super::SandboxTransformRequest {
            spec: super::CommandSpec {
                program: "true".to_string(),
                args: Vec::new(),
                cwd: cwd.clone(),
                env: HashMap::new(),
                expiration: crate::exec::ExecExpiration::DefaultTimeout,
                capture_policy: crate::exec::ExecCapturePolicy::ShellTool,
                sandbox_permissions: super::SandboxPermissions::UseDefault,
                additional_permissions: None,
                justification: None,
            },
            policy: &SandboxPolicy::ExternalSandbox {
                network_access: crate::protocol::NetworkAccess::Restricted,
            },
            file_system_policy: &FileSystemSandboxPolicy::unrestricted(),
            network_policy: NetworkSandboxPolicy::Restricted,
            sandbox: SandboxType::None,
            enforce_managed_network: false,
            network: None,
            sandbox_policy_cwd: cwd.as_path(),
            #[cfg(target_os = "macos")]
            macos_seatbelt_profile_extensions: None,
            codex_linux_sandbox_exe: None,
            use_legacy_landlock: false,
            windows_sandbox_level: WindowsSandboxLevel::Disabled,
            windows_sandbox_private_desktop: false,
        })
        .expect("transform");

    assert_eq!(
        exec_request.file_system_sandbox_policy,
        FileSystemSandboxPolicy::unrestricted()
    );
    assert_eq!(
        exec_request.network_sandbox_policy,
        NetworkSandboxPolicy::Restricted
    );
}

#[cfg(target_os = "macos")]
#[test]
fn transform_wraps_command_for_seatbelt_execution() {
    let manager = SandboxManager::new();
    let cwd = std::env::current_dir().expect("current dir");
    let policy = SandboxPolicy::new_read_only_policy();
    let file_system_policy =
        FileSystemSandboxPolicy::from_legacy_sandbox_policy(&policy, cwd.as_path());
    let exec_request = manager
        .transform(super::SandboxTransformRequest {
            spec: sandbox_command_spec(
                vec!["/bin/echo".to_string(), "hello".to_string()],
                cwd.clone(),
                HashMap::new(),
            ),
            policy: &policy,
            file_system_policy: &file_system_policy,
            network_policy: NetworkSandboxPolicy::from(&policy),
            sandbox: SandboxType::MacosSeatbelt,
            enforce_managed_network: false,
            network: None,
            sandbox_policy_cwd: cwd.as_path(),
            macos_seatbelt_profile_extensions: None,
            codex_linux_sandbox_exe: None,
            use_legacy_landlock: false,
            windows_sandbox_level: WindowsSandboxLevel::Disabled,
            windows_sandbox_private_desktop: false,
        })
        .expect("transform");

    assert_eq!(
        exec_request.command.first().map(String::as_str),
        Some(codex_sandboxing::seatbelt::MACOS_PATH_TO_SEATBELT_EXECUTABLE)
    );
    assert_eq!(exec_request.arg0, None);
    assert_eq!(
        exec_request
            .env
            .get(crate::spawn::CODEX_SANDBOX_ENV_VAR)
            .map(String::as_str),
        Some("seatbelt")
    );
    assert_eq!(
        exec_request
            .env
            .get(crate::spawn::CODEX_SANDBOX_NETWORK_DISABLED_ENV_VAR)
            .map(String::as_str),
        Some("1")
    );
}

#[cfg(target_os = "linux")]
#[test]
fn transform_wraps_command_for_linux_sandbox_execution() {
    let manager = SandboxManager::new();
    let cwd = std::env::current_dir().expect("current dir");
    let policy = SandboxPolicy::new_read_only_policy();
    let file_system_policy =
        FileSystemSandboxPolicy::from_legacy_sandbox_policy(&policy, cwd.as_path());
    let sandbox_exe = PathBuf::from("/tmp/codex-linux-sandbox");
    let exec_request = manager
        .transform(super::SandboxTransformRequest {
            spec: sandbox_command_spec(
                vec!["/bin/echo".to_string(), "hello".to_string()],
                cwd.clone(),
                HashMap::new(),
            ),
            policy: &policy,
            file_system_policy: &file_system_policy,
            network_policy: NetworkSandboxPolicy::from(&policy),
            sandbox: SandboxType::LinuxSeccomp,
            enforce_managed_network: false,
            network: None,
            sandbox_policy_cwd: cwd.as_path(),
            codex_linux_sandbox_exe: Some(&sandbox_exe),
            use_legacy_landlock: false,
            windows_sandbox_level: WindowsSandboxLevel::Disabled,
            windows_sandbox_private_desktop: false,
        })
        .expect("transform");

    assert_eq!(
        exec_request.command.first().map(String::as_str),
        sandbox_exe.to_str()
    );
    assert_eq!(
        exec_request
            .env
            .get(crate::spawn::CODEX_SANDBOX_NETWORK_DISABLED_ENV_VAR)
            .map(String::as_str),
        Some("1")
    );
    assert_eq!(exec_request.arg0.as_deref(), Some("codex-linux-sandbox"));
}

#[test]
fn normalize_additional_permissions_preserves_network() {
    let temp_dir = TempDir::new().expect("create temp dir");
    let path = AbsolutePathBuf::from_absolute_path(
        canonicalize(temp_dir.path()).expect("canonicalize temp dir"),
    )
    .expect("absolute temp dir");
    let permissions = normalize_additional_permissions(PermissionProfile {
        network: Some(NetworkPermissions {
            enabled: Some(true),
        }),
        file_system: Some(FileSystemPermissions {
            read: Some(vec![path.clone()]),
            write: Some(vec![path.clone()]),
        }),
        ..Default::default()
    })
    .expect("permissions");

    assert_eq!(
        permissions.network,
        Some(NetworkPermissions {
            enabled: Some(true),
        })
    );
    assert_eq!(
        permissions.file_system,
        Some(FileSystemPermissions {
            read: Some(vec![path.clone()]),
            write: Some(vec![path]),
        })
    );
}

#[cfg(unix)]
#[test]
fn normalize_additional_permissions_canonicalizes_symlinked_write_paths() {
    let temp_dir = TempDir::new().expect("create temp dir");
    let real_root = temp_dir.path().join("real");
    let link_root = temp_dir.path().join("link");
    let write_dir = real_root.join("write");
    std::fs::create_dir_all(&write_dir).expect("create write dir");
    symlink_dir(&real_root, &link_root).expect("create symlinked root");

    let link_write_dir =
        AbsolutePathBuf::from_absolute_path(link_root.join("write")).expect("link write dir");
    let expected_write_dir = AbsolutePathBuf::from_absolute_path(
        write_dir.canonicalize().expect("canonicalize write dir"),
    )
    .expect("absolute canonical write dir");

    let permissions = normalize_additional_permissions(PermissionProfile {
        file_system: Some(FileSystemPermissions {
            read: Some(vec![]),
            write: Some(vec![link_write_dir]),
        }),
        ..Default::default()
    })
    .expect("permissions");

    assert_eq!(
        permissions.file_system,
        Some(FileSystemPermissions {
            read: Some(vec![]),
            write: Some(vec![expected_write_dir]),
        })
    );
}

#[test]
fn normalize_additional_permissions_drops_empty_nested_profiles() {
    let permissions = normalize_additional_permissions(PermissionProfile {
        network: Some(NetworkPermissions { enabled: None }),
        file_system: Some(FileSystemPermissions {
            read: None,
            write: None,
        }),
        macos: None,
    })
    .expect("permissions");

    assert_eq!(permissions, PermissionProfile::default());
}

#[cfg(target_os = "macos")]
#[test]
fn normalize_additional_permissions_preserves_default_macos_preferences_permission() {
    let permissions = normalize_additional_permissions(PermissionProfile {
        macos: Some(MacOsSeatbeltProfileExtensions::default()),
        ..Default::default()
    })
    .expect("permissions");

    assert_eq!(
        permissions,
        PermissionProfile {
            macos: Some(MacOsSeatbeltProfileExtensions::default()),
            ..Default::default()
        }
    );
}

#[cfg(target_os = "macos")]
#[test]
fn intersect_permission_profiles_preserves_default_macos_grants() {
    let requested = PermissionProfile {
        file_system: Some(FileSystemPermissions {
            read: Some(Vec::from(["/tmp/requested"
                .try_into()
                .expect("absolute path")])),
            write: None,
        }),
        macos: Some(MacOsSeatbeltProfileExtensions {
            macos_preferences: MacOsPreferencesPermission::ReadWrite,
            macos_automation: MacOsAutomationPermission::BundleIds(vec![
                "com.apple.Notes".to_string(),
            ]),
            macos_launch_services: false,
            macos_accessibility: true,
            macos_calendar: true,
            macos_reminders: false,
            macos_contacts: MacOsContactsPermission::None,
        }),
        ..Default::default()
    };
    let granted = PermissionProfile {
        file_system: Some(FileSystemPermissions {
            read: Some(Vec::new()),
            write: None,
        }),
        macos: Some(MacOsSeatbeltProfileExtensions::default()),
        ..Default::default()
    };

    assert_eq!(
        intersect_permission_profiles(requested, granted),
        PermissionProfile {
            macos: Some(MacOsSeatbeltProfileExtensions::default()),
            ..Default::default()
        }
    );
}

#[cfg(target_os = "macos")]
#[test]
fn normalize_additional_permissions_preserves_macos_permissions() {
    let permissions = normalize_additional_permissions(PermissionProfile {
        macos: Some(MacOsSeatbeltProfileExtensions {
            macos_preferences: MacOsPreferencesPermission::ReadWrite,
            macos_automation: MacOsAutomationPermission::BundleIds(vec![
                "com.apple.Notes".to_string(),
            ]),
            macos_launch_services: true,
            macos_accessibility: true,
            macos_calendar: true,
            macos_reminders: false,
            macos_contacts: MacOsContactsPermission::None,
        }),
        ..Default::default()
    })
    .expect("permissions");

    assert_eq!(
        permissions.macos,
        Some(MacOsSeatbeltProfileExtensions {
            macos_preferences: MacOsPreferencesPermission::ReadWrite,
            macos_automation: MacOsAutomationPermission::BundleIds(vec![
                "com.apple.Notes".to_string(),
            ]),
            macos_launch_services: true,
            macos_accessibility: true,
            macos_calendar: true,
            macos_reminders: false,
            macos_contacts: MacOsContactsPermission::None,
        })
    );
}

#[test]
fn read_only_additional_permissions_can_enable_network_without_writes() {
    let temp_dir = TempDir::new().expect("create temp dir");
    let path = AbsolutePathBuf::from_absolute_path(
        canonicalize(temp_dir.path()).expect("canonicalize temp dir"),
    )
    .expect("absolute temp dir");
    let policy = sandbox_policy_with_additional_permissions(
        &SandboxPolicy::ReadOnly {
            access: ReadOnlyAccess::Restricted {
                include_platform_defaults: true,
                readable_roots: vec![path.clone()],
            },
            network_access: false,
        },
        &PermissionProfile {
            network: Some(NetworkPermissions {
                enabled: Some(true),
            }),
            file_system: Some(FileSystemPermissions {
                read: Some(vec![path.clone()]),
                write: Some(Vec::new()),
            }),
            ..Default::default()
        },
    );

    assert_eq!(
        policy,
        SandboxPolicy::ReadOnly {
            access: ReadOnlyAccess::Restricted {
                include_platform_defaults: true,
                readable_roots: vec![path],
            },
            network_access: true,
        }
    );
}
#[cfg(target_os = "macos")]
#[test]
fn effective_permissions_merge_macos_extensions_with_additional_permissions() {
    let temp_dir = TempDir::new().expect("create temp dir");
    let path = AbsolutePathBuf::from_absolute_path(
        canonicalize(temp_dir.path()).expect("canonicalize temp dir"),
    )
    .expect("absolute temp dir");
    let effective_permissions = EffectiveSandboxPermissions::new(
        &SandboxPolicy::ReadOnly {
            access: ReadOnlyAccess::Restricted {
                include_platform_defaults: true,
                readable_roots: vec![path.clone()],
            },
            network_access: false,
        },
        Some(&MacOsSeatbeltProfileExtensions {
            macos_preferences: MacOsPreferencesPermission::ReadOnly,
            macos_automation: MacOsAutomationPermission::BundleIds(vec![
                "com.apple.Calendar".to_string(),
            ]),
            macos_launch_services: false,
            macos_accessibility: false,
            macos_calendar: false,
            macos_reminders: false,
            macos_contacts: MacOsContactsPermission::None,
        }),
        Some(&PermissionProfile {
            file_system: Some(FileSystemPermissions {
                read: Some(vec![path]),
                write: Some(Vec::new()),
            }),
            macos: Some(MacOsSeatbeltProfileExtensions {
                macos_preferences: MacOsPreferencesPermission::ReadWrite,
                macos_automation: MacOsAutomationPermission::BundleIds(vec![
                    "com.apple.Notes".to_string(),
                ]),
                macos_launch_services: true,
                macos_accessibility: true,
                macos_calendar: true,
                macos_reminders: false,
                macos_contacts: MacOsContactsPermission::None,
            }),
            ..Default::default()
        }),
    );

    assert_eq!(
        effective_permissions.macos_seatbelt_profile_extensions,
        Some(MacOsSeatbeltProfileExtensions {
            macos_preferences: MacOsPreferencesPermission::ReadWrite,
            macos_automation: MacOsAutomationPermission::BundleIds(vec![
                "com.apple.Calendar".to_string(),
                "com.apple.Notes".to_string(),
            ]),
            macos_launch_services: true,
            macos_accessibility: true,
            macos_calendar: true,
            macos_reminders: false,
            macos_contacts: MacOsContactsPermission::None,
        })
    );
}

#[test]
fn external_sandbox_additional_permissions_can_enable_network() {
    let temp_dir = TempDir::new().expect("create temp dir");
    let path = AbsolutePathBuf::from_absolute_path(
        canonicalize(temp_dir.path()).expect("canonicalize temp dir"),
    )
    .expect("absolute temp dir");
    let policy = sandbox_policy_with_additional_permissions(
        &SandboxPolicy::ExternalSandbox {
            network_access: NetworkAccess::Restricted,
        },
        &PermissionProfile {
            network: Some(NetworkPermissions {
                enabled: Some(true),
            }),
            file_system: Some(FileSystemPermissions {
                read: Some(vec![path]),
                write: Some(Vec::new()),
            }),
            ..Default::default()
        },
    );

    assert_eq!(
        policy,
        SandboxPolicy::ExternalSandbox {
            network_access: NetworkAccess::Enabled,
        }
    );
}

#[test]
fn transform_additional_permissions_enable_network_for_external_sandbox() {
    let manager = SandboxManager::new();
    let cwd = std::env::current_dir().expect("current dir");
    let temp_dir = TempDir::new().expect("create temp dir");
    let path = AbsolutePathBuf::from_absolute_path(
        canonicalize(temp_dir.path()).expect("canonicalize temp dir"),
    )
    .expect("absolute temp dir");
    let exec_request = manager
        .transform(super::SandboxTransformRequest {
            spec: super::CommandSpec {
                program: "true".to_string(),
                args: Vec::new(),
                cwd: cwd.clone(),
                env: HashMap::new(),
                expiration: crate::exec::ExecExpiration::DefaultTimeout,
                capture_policy: crate::exec::ExecCapturePolicy::ShellTool,
                sandbox_permissions: super::SandboxPermissions::WithAdditionalPermissions,
                additional_permissions: Some(PermissionProfile {
                    network: Some(NetworkPermissions {
                        enabled: Some(true),
                    }),
                    file_system: Some(FileSystemPermissions {
                        read: Some(vec![path]),
                        write: Some(Vec::new()),
                    }),
                    ..Default::default()
                }),
                justification: None,
            },
            policy: &SandboxPolicy::ExternalSandbox {
                network_access: NetworkAccess::Restricted,
            },
            file_system_policy: &FileSystemSandboxPolicy::unrestricted(),
            network_policy: NetworkSandboxPolicy::Restricted,
            sandbox: SandboxType::None,
            enforce_managed_network: false,
            network: None,
            sandbox_policy_cwd: cwd.as_path(),
            #[cfg(target_os = "macos")]
            macos_seatbelt_profile_extensions: None,
            codex_linux_sandbox_exe: None,
            use_legacy_landlock: false,
            windows_sandbox_level: WindowsSandboxLevel::Disabled,
            windows_sandbox_private_desktop: false,
        })
        .expect("transform");

    assert_eq!(
        exec_request.sandbox_policy,
        SandboxPolicy::ExternalSandbox {
            network_access: NetworkAccess::Enabled,
        }
    );
    assert_eq!(
        exec_request.network_sandbox_policy,
        NetworkSandboxPolicy::Enabled
    );
}

#[test]
fn transform_additional_permissions_preserves_denied_entries() {
    let manager = SandboxManager::new();
    let cwd = std::env::current_dir().expect("current dir");
    let temp_dir = TempDir::new().expect("create temp dir");
    let workspace_root = AbsolutePathBuf::from_absolute_path(
        canonicalize(temp_dir.path()).expect("canonicalize temp dir"),
    )
    .expect("absolute temp dir");
    let allowed_path = workspace_root.join("allowed").expect("allowed path");
    let denied_path = workspace_root.join("denied").expect("denied path");
    let exec_request = manager
        .transform(super::SandboxTransformRequest {
            spec: super::CommandSpec {
                program: "true".to_string(),
                args: Vec::new(),
                cwd: cwd.clone(),
                env: HashMap::new(),
                expiration: crate::exec::ExecExpiration::DefaultTimeout,
                capture_policy: crate::exec::ExecCapturePolicy::ShellTool,
                sandbox_permissions: super::SandboxPermissions::WithAdditionalPermissions,
                additional_permissions: Some(PermissionProfile {
                    file_system: Some(FileSystemPermissions {
                        read: None,
                        write: Some(vec![allowed_path.clone()]),
                    }),
                    ..Default::default()
                }),
                justification: None,
            },
            policy: &SandboxPolicy::ReadOnly {
                access: ReadOnlyAccess::FullAccess,
                network_access: false,
            },
            file_system_policy: &FileSystemSandboxPolicy::restricted(vec![
                FileSystemSandboxEntry {
                    path: FileSystemPath::Special {
                        value: FileSystemSpecialPath::Root,
                    },
                    access: FileSystemAccessMode::Read,
                },
                FileSystemSandboxEntry {
                    path: FileSystemPath::Path {
                        path: denied_path.clone(),
                    },
                    access: FileSystemAccessMode::None,
                },
            ]),
            network_policy: NetworkSandboxPolicy::Restricted,
            sandbox: SandboxType::None,
            enforce_managed_network: false,
            network: None,
            sandbox_policy_cwd: cwd.as_path(),
            #[cfg(target_os = "macos")]
            macos_seatbelt_profile_extensions: None,
            codex_linux_sandbox_exe: None,
            use_legacy_landlock: false,
            windows_sandbox_level: WindowsSandboxLevel::Disabled,
            windows_sandbox_private_desktop: false,
        })
        .expect("transform");

    assert_eq!(
        exec_request.file_system_sandbox_policy,
        FileSystemSandboxPolicy::restricted(vec![
            FileSystemSandboxEntry {
                path: FileSystemPath::Special {
                    value: FileSystemSpecialPath::Root,
                },
                access: FileSystemAccessMode::Read,
            },
            FileSystemSandboxEntry {
                path: FileSystemPath::Path { path: denied_path },
                access: FileSystemAccessMode::None,
            },
            FileSystemSandboxEntry {
                path: FileSystemPath::Path { path: allowed_path },
                access: FileSystemAccessMode::Write,
            },
        ])
    );
    assert_eq!(
        exec_request.network_sandbox_policy,
        NetworkSandboxPolicy::Restricted
    );
}

#[test]
fn merge_file_system_policy_with_additional_permissions_preserves_unreadable_roots() {
    let temp_dir = TempDir::new().expect("create temp dir");
    let cwd = AbsolutePathBuf::from_absolute_path(
        canonicalize(temp_dir.path()).expect("canonicalize temp dir"),
    )
    .expect("absolute temp dir");
    let allowed_path = cwd.join("allowed").expect("allowed path");
    let denied_path = cwd.join("denied").expect("denied path");
    let merged_policy = merge_file_system_policy_with_additional_permissions(
        &FileSystemSandboxPolicy::restricted(vec![
            FileSystemSandboxEntry {
                path: FileSystemPath::Special {
                    value: FileSystemSpecialPath::Root,
                },
                access: FileSystemAccessMode::Read,
            },
            FileSystemSandboxEntry {
                path: FileSystemPath::Path {
                    path: denied_path.clone(),
                },
                access: FileSystemAccessMode::None,
            },
        ]),
        vec![allowed_path.clone()],
        Vec::new(),
    );

    assert_eq!(
        merged_policy.entries.contains(&FileSystemSandboxEntry {
            path: FileSystemPath::Path { path: denied_path },
            access: FileSystemAccessMode::None,
        }),
        true
    );
    assert_eq!(
        merged_policy.entries.contains(&FileSystemSandboxEntry {
            path: FileSystemPath::Path { path: allowed_path },
            access: FileSystemAccessMode::Read,
        }),
        true
    );
}

#[test]
fn effective_file_system_sandbox_policy_returns_base_policy_without_additional_permissions() {
    let temp_dir = TempDir::new().expect("create temp dir");
    let cwd = AbsolutePathBuf::from_absolute_path(
        canonicalize(temp_dir.path()).expect("canonicalize temp dir"),
    )
    .expect("absolute temp dir");
    let denied_path = cwd.join("denied").expect("denied path");
    let base_policy = FileSystemSandboxPolicy::restricted(vec![
        FileSystemSandboxEntry {
            path: FileSystemPath::Special {
                value: FileSystemSpecialPath::Root,
            },
            access: FileSystemAccessMode::Read,
        },
        FileSystemSandboxEntry {
            path: FileSystemPath::Path { path: denied_path },
            access: FileSystemAccessMode::None,
        },
    ]);

    let effective_policy = effective_file_system_sandbox_policy(&base_policy, None);

    assert_eq!(effective_policy, base_policy);
}

#[test]
fn effective_file_system_sandbox_policy_merges_additional_write_roots() {
    let temp_dir = TempDir::new().expect("create temp dir");
    let cwd = AbsolutePathBuf::from_absolute_path(
        canonicalize(temp_dir.path()).expect("canonicalize temp dir"),
    )
    .expect("absolute temp dir");
    let allowed_path = cwd.join("allowed").expect("allowed path");
    let denied_path = cwd.join("denied").expect("denied path");
    let base_policy = FileSystemSandboxPolicy::restricted(vec![
        FileSystemSandboxEntry {
            path: FileSystemPath::Special {
                value: FileSystemSpecialPath::Root,
            },
            access: FileSystemAccessMode::Read,
        },
        FileSystemSandboxEntry {
            path: FileSystemPath::Path {
                path: denied_path.clone(),
            },
            access: FileSystemAccessMode::None,
        },
    ]);
    let additional_permissions = PermissionProfile {
        file_system: Some(FileSystemPermissions {
            read: Some(vec![]),
            write: Some(vec![allowed_path.clone()]),
        }),
        ..Default::default()
    };

    let effective_policy =
        effective_file_system_sandbox_policy(&base_policy, Some(&additional_permissions));

    assert_eq!(
        effective_policy.entries.contains(&FileSystemSandboxEntry {
            path: FileSystemPath::Path { path: denied_path },
            access: FileSystemAccessMode::None,
        }),
        true
    );
    assert_eq!(
        effective_policy.entries.contains(&FileSystemSandboxEntry {
            path: FileSystemPath::Path { path: allowed_path },
            access: FileSystemAccessMode::Write,
        }),
        true
    );
}

#[cfg(unix)]
#[tokio::test]
async fn python_multiprocessing_lock_works_under_platform_sandbox() {
    skip_if_sandbox!();

    #[cfg(target_os = "linux")]
    let codex_linux_sandbox_exe = codex_linux_sandbox_exe_or_skip!();
    #[cfg(not(target_os = "linux"))]
    let codex_linux_sandbox_exe: Option<PathBuf> = None;

    #[cfg(target_os = "linux")]
    if !linux_sandbox_is_enforceable(
        codex_linux_sandbox_exe
            .as_ref()
            .expect("linux sandbox binary should be set"),
    )
    .await
    {
        eprintln!("Skipping test: Landlock is not enforceable on this host.");
        return;
    }

    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("python3 not found in PATH, skipping test.");
        return;
    }

    #[cfg(target_os = "macos")]
    let writable_roots = Vec::<AbsolutePathBuf>::new();
    #[cfg(target_os = "linux")]
    let writable_roots: Vec<AbsolutePathBuf> = vec!["/dev/shm".try_into().expect("/dev/shm")];

    let policy = SandboxPolicy::WorkspaceWrite {
        writable_roots,
        read_only_access: Default::default(),
        network_access: false,
        exclude_tmpdir_env_var: false,
        exclude_slash_tmp: false,
    };
    let python_code = r#"import multiprocessing
from multiprocessing import Lock, Process

def f(lock):
    with lock:
        print("Lock acquired in child process")

if __name__ == '__main__':
    lock = Lock()
    p = Process(target=f, args=(lock,))
    p.start()
    p.join()
"#;
    let command_cwd = std::env::current_dir().expect("current dir");
    let output = execute_command_under_platform_sandbox(
        vec![
            "python3".to_string(),
            "-c".to_string(),
            python_code.to_string(),
        ],
        command_cwd,
        &policy,
        HashMap::new(),
        codex_linux_sandbox_exe.as_ref(),
    )
    .await
    .expect("should execute python under sandbox");

    assert_eq!(
        output.exit_code, 0,
        "python exited with {}\nstdout: {}\nstderr: {}",
        output.exit_code, output.stdout.text, output.stderr.text
    );
}

#[cfg(unix)]
#[tokio::test]
async fn python_getpwuid_works_under_platform_sandbox() {
    skip_if_sandbox!();

    #[cfg(target_os = "linux")]
    let codex_linux_sandbox_exe = codex_linux_sandbox_exe_or_skip!();
    #[cfg(not(target_os = "linux"))]
    let codex_linux_sandbox_exe: Option<PathBuf> = None;

    #[cfg(target_os = "linux")]
    if !linux_sandbox_is_enforceable(
        codex_linux_sandbox_exe
            .as_ref()
            .expect("linux sandbox binary should be set"),
    )
    .await
    {
        eprintln!("Skipping test: Landlock is not enforceable on this host.");
        return;
    }

    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("python3 not found in PATH, skipping test.");
        return;
    }

    let command_cwd = std::env::current_dir().expect("current dir");
    let output = execute_command_under_platform_sandbox(
        vec![
            "python3".to_string(),
            "-c".to_string(),
            "import pwd, os; print(pwd.getpwuid(os.getuid()))".to_string(),
        ],
        command_cwd,
        &SandboxPolicy::new_read_only_policy(),
        HashMap::new(),
        codex_linux_sandbox_exe.as_ref(),
    )
    .await
    .expect("should execute python under sandbox");

    assert_eq!(
        output.exit_code, 0,
        "python exited with {}\nstdout: {}\nstderr: {}",
        output.exit_code, output.stdout.text, output.stderr.text
    );
    assert!(
        !output.stdout.text.trim().is_empty(),
        "expected pwd.getpwuid output, stderr: {}",
        output.stderr.text
    );
}
