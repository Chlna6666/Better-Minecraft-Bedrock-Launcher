use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
};

use egpui_manifest::{AppManifest, PackageFormat};
use thiserror::Error;

/// Operating-system family required by a package format.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostPlatform {
    /// Microsoft Windows.
    Windows,
    /// Apple macOS.
    MacOs,
    /// Desktop Linux.
    Linux,
    /// A host for which no native package backend is implemented.
    Unsupported,
}

impl HostPlatform {
    /// Returns the platform on which this build helper is executing.
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else {
            Self::Unsupported
        }
    }
}

/// One resource copied beside the application executable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BundleResource {
    /// Existing source file.
    pub source: PathBuf,
    /// Portable relative path beneath the bundle resource directory.
    pub logical_path: String,
}

/// Inputs required to build one package plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BundleRequest {
    /// Compiled executable for the target platform.
    pub executable: PathBuf,
    /// Directory that receives staging data and final artifacts.
    pub output_directory: PathBuf,
    /// Requested package format.
    pub format: PackageFormat,
    /// Files declared as bundled rather than embedded.
    pub bundled_resources: Vec<BundleResource>,
}

/// One deterministic filesystem or native-tool action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BundleStep {
    /// Recreates an isolated staging directory.
    ResetDirectory(PathBuf),
    /// Creates a directory and missing parents.
    CreateDirectory(PathBuf),
    /// Copies one file, creating the destination parent.
    CopyFile {
        /// Existing input file.
        source: PathBuf,
        /// Output file path.
        destination: PathBuf,
    },
    /// Writes UTF-8 metadata.
    WriteText {
        /// Output file path.
        destination: PathBuf,
        /// Complete file contents.
        contents: String,
    },
    /// Marks a generated launcher executable on Unix.
    SetExecutable(PathBuf),
    /// Runs a platform packaging or signing tool.
    RunTool {
        /// Executable resolved through `PATH`.
        program: String,
        /// Argument vector passed without shell interpolation.
        arguments: Vec<String>,
        /// Optional working directory.
        current_directory: Option<PathBuf>,
    },
}

/// Complete plan for one package artifact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BundlePlan {
    /// Package format.
    pub format: PackageFormat,
    /// Isolated staging directory.
    pub staging_directory: PathBuf,
    /// Expected final artifact or application directory.
    pub artifact_path: PathBuf,
    /// Ordered actions.
    pub steps: Vec<BundleStep>,
}

/// Failure while deriving a package plan.
#[derive(Debug, Error)]
pub enum BundlePlanError {
    /// The manifest did not opt into the requested format.
    #[error("package target `{0:?}` is not declared by the application manifest")]
    TargetNotDeclared(PackageFormat),
    /// Native packaging must run on the matching operating-system family.
    #[error("bundle format `{format:?}` cannot be built on host `{host:?}`")]
    UnsupportedHost {
        /// Requested package format.
        format: PackageFormat,
        /// Current host platform.
        host: HostPlatform,
    },
    /// A required package field was absent.
    #[error("bundle format `{format:?}` requires manifest field `{field}`")]
    MissingConfiguration {
        /// Requested package format.
        format: PackageFormat,
        /// Dotted manifest field path.
        field: &'static str,
    },
    /// The executable path was absent or not a file.
    #[error("bundle executable `{0}` does not exist or is not a file")]
    InvalidExecutable(PathBuf),
    /// A bundled resource source was absent or not a file.
    #[error("bundled resource source `{0}` does not exist or is not a file")]
    InvalidResourceSource(PathBuf),
    /// A bundled resource path attempted traversal or used platform separators.
    #[error("bundled resource path `{0}` is not portable")]
    InvalidResourcePath(String),
    /// Two bundled files have the same case-insensitive logical path.
    #[error("bundled resource path collision: `{0}`")]
    ResourceCollision(String),
    /// A product version cannot be represented by the target package format.
    #[error("application version `{version}` cannot be represented by `{format:?}`")]
    UnsupportedVersion {
        /// Requested package format.
        format: PackageFormat,
        /// Application SemVer.
        version: String,
    },
}

/// Replaceable package-plan backend.
pub trait BundlerBackend {
    /// Returns formats supported by the current backend and host.
    fn supported_formats(&self) -> BTreeSet<PackageFormat>;

    /// Produces a deterministic plan without mutating the filesystem.
    ///
    /// # Errors
    ///
    /// Returns a target, host, input, or manifest configuration error.
    fn plan(
        &self,
        manifest: &AppManifest,
        request: &BundleRequest,
    ) -> Result<BundlePlan, BundlePlanError>;
}

/// Native package backend using platform-standard directory layouts and tools.
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeBundlerBackend;

impl BundlerBackend for NativeBundlerBackend {
    fn supported_formats(&self) -> BTreeSet<PackageFormat> {
        formats_for_host(HostPlatform::current())
    }

    fn plan(
        &self,
        manifest: &AppManifest,
        request: &BundleRequest,
    ) -> Result<BundlePlan, BundlePlanError> {
        if !manifest.bundle.targets.contains(&request.format) {
            return Err(BundlePlanError::TargetNotDeclared(request.format));
        }
        let host = HostPlatform::current();
        if !formats_for_host(host).contains(&request.format) {
            return Err(BundlePlanError::UnsupportedHost {
                format: request.format,
                host,
            });
        }
        if !request.executable.is_file() {
            return Err(BundlePlanError::InvalidExecutable(
                request.executable.clone(),
            ));
        }
        validate_resources(&request.bundled_resources)?;

        match request.format {
            PackageFormat::WindowsPortable => plan_windows_portable(manifest, request),
            PackageFormat::WindowsMsix => plan_windows_msix(manifest, request),
            PackageFormat::WindowsMsi => plan_windows_msi(manifest, request),
            PackageFormat::WindowsNsis => plan_windows_nsis(manifest, request),
            PackageFormat::MacOsApp => plan_macos_app(manifest, request),
            PackageFormat::MacOsDmg => plan_macos_dmg(manifest, request),
            PackageFormat::LinuxAppImage => plan_linux_app_image(manifest, request),
            PackageFormat::LinuxDeb => plan_linux_deb(manifest, request),
            PackageFormat::LinuxRpm => plan_linux_rpm(manifest, request),
        }
    }
}

/// Failure while executing an already validated bundle plan.
#[derive(Debug, Error)]
pub enum BundleExecutionError {
    /// A staging path was too broad to reset safely.
    #[error("refusing to reset unsafe staging directory `{0}`")]
    UnsafeReset(PathBuf),
    /// A filesystem action failed.
    #[error("bundle filesystem operation failed for `{path}`: {source}")]
    Filesystem {
        /// Affected path.
        path: PathBuf,
        /// Underlying IO error.
        source: std::io::Error,
    },
    /// A native packaging tool could not be started.
    #[error("failed to start bundle tool `{program}`: {source}")]
    ToolStart {
        /// Tool executable.
        program: String,
        /// Underlying process error.
        source: std::io::Error,
    },
    /// A native packaging tool returned a failure status.
    #[error("bundle tool `{program}` exited unsuccessfully with status {status}")]
    ToolFailed {
        /// Tool executable.
        program: String,
        /// Portable status string.
        status: String,
    },
    /// Every step completed but the declared artifact is absent.
    #[error("bundle completed without producing `{0}`")]
    ArtifactMissing(PathBuf),
}

/// Executes a package plan.
pub trait BundleExecutor {
    /// Runs every plan step in order and verifies the artifact.
    ///
    /// # Errors
    ///
    /// Returns a filesystem, tool, status, or artifact error.
    fn execute(&self, plan: &BundlePlan) -> Result<PathBuf, BundleExecutionError>;
}

/// Process-based executor for [`NativeBundlerBackend`] plans.
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeBundleExecutor;

impl BundleExecutor for NativeBundleExecutor {
    fn execute(&self, plan: &BundlePlan) -> Result<PathBuf, BundleExecutionError> {
        for step in &plan.steps {
            execute_step(step)?;
        }
        if !plan.artifact_path.exists() {
            return Err(BundleExecutionError::ArtifactMissing(
                plan.artifact_path.clone(),
            ));
        }
        Ok(plan.artifact_path.clone())
    }
}

fn formats_for_host(host: HostPlatform) -> BTreeSet<PackageFormat> {
    match host {
        HostPlatform::Windows => [
            PackageFormat::WindowsPortable,
            PackageFormat::WindowsMsix,
            PackageFormat::WindowsMsi,
            PackageFormat::WindowsNsis,
        ]
        .into_iter()
        .collect(),
        HostPlatform::MacOs => [PackageFormat::MacOsApp, PackageFormat::MacOsDmg]
            .into_iter()
            .collect(),
        HostPlatform::Linux => [
            PackageFormat::LinuxAppImage,
            PackageFormat::LinuxDeb,
            PackageFormat::LinuxRpm,
        ]
        .into_iter()
        .collect(),
        HostPlatform::Unsupported => BTreeSet::new(),
    }
}

fn validate_resources(resources: &[BundleResource]) -> Result<(), BundlePlanError> {
    let mut paths = BTreeSet::new();
    for resource in resources {
        if !resource.source.is_file() {
            return Err(BundlePlanError::InvalidResourceSource(
                resource.source.clone(),
            ));
        }
        let path = Path::new(&resource.logical_path);
        if resource.logical_path.is_empty()
            || resource.logical_path.contains('\\')
            || path.is_absolute()
            || path.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::CurDir | Component::Prefix(_)
                )
            })
        {
            return Err(BundlePlanError::InvalidResourcePath(
                resource.logical_path.clone(),
            ));
        }
        if !paths.insert(resource.logical_path.to_lowercase()) {
            return Err(BundlePlanError::ResourceCollision(
                resource.logical_path.clone(),
            ));
        }
    }
    Ok(())
}

fn plan_windows_portable(
    manifest: &AppManifest,
    request: &BundleRequest,
) -> Result<BundlePlan, BundlePlanError> {
    let binary = windows_binary_name(manifest);
    let staging = request
        .output_directory
        .join(format!("{}-portable", binary_name(manifest)));
    let mut steps = common_staging_steps(request, &staging, &binary, "resources");
    steps.push(BundleStep::WriteText {
        destination: staging.join("egpui-application.json"),
        contents: application_metadata_json(manifest, request.format),
    });
    Ok(BundlePlan {
        format: request.format,
        artifact_path: staging.clone(),
        staging_directory: staging,
        steps,
    })
}

fn plan_windows_msix(
    manifest: &AppManifest,
    request: &BundleRequest,
) -> Result<BundlePlan, BundlePlanError> {
    let publisher = manifest
        .bundle
        .windows
        .publisher_identity
        .as_deref()
        .ok_or(BundlePlanError::MissingConfiguration {
            format: request.format,
            field: "bundle.windows.publisher_identity",
        })?;
    let icon =
        manifest
            .bundle
            .icons
            .source
            .as_ref()
            .ok_or(BundlePlanError::MissingConfiguration {
                format: request.format,
                field: "bundle.icons.source",
            })?;
    let version = windows_package_version(manifest, request.format)?;
    let binary = windows_binary_name(manifest);
    let staging = request.output_directory.join("msix");
    let artifact = request.output_directory.join(format!(
        "{}-{}-windows.msix",
        binary_name(manifest),
        manifest.application.version
    ));
    let mut steps = common_staging_steps(request, &staging, &binary, "resources");
    steps.push(BundleStep::CreateDirectory(staging.join("Assets")));
    for name in [
        "StoreLogo.png",
        "Square44x44Logo.png",
        "Square150x150Logo.png",
    ] {
        steps.push(BundleStep::CopyFile {
            source: icon.clone(),
            destination: staging.join("Assets").join(name),
        });
    }
    steps.push(BundleStep::WriteText {
        destination: staging.join("AppxManifest.xml"),
        contents: windows_appx_manifest(manifest, publisher, &version, &binary),
    });
    steps.push(run_tool(
        "makeappx",
        [
            "pack".to_owned(),
            "/d".to_owned(),
            path_string(&staging),
            "/p".to_owned(),
            path_string(&artifact),
            "/o".to_owned(),
        ],
    ));
    append_windows_signing(manifest, &artifact, &mut steps);
    Ok(BundlePlan {
        format: request.format,
        staging_directory: staging,
        artifact_path: artifact,
        steps,
    })
}

fn plan_windows_msi(
    manifest: &AppManifest,
    request: &BundleRequest,
) -> Result<BundlePlan, BundlePlanError> {
    let upgrade_code = manifest.bundle.windows.upgrade_code.as_deref().ok_or(
        BundlePlanError::MissingConfiguration {
            format: request.format,
            field: "bundle.windows.upgrade_code",
        },
    )?;
    let binary = windows_binary_name(manifest);
    let staging = request.output_directory.join("msi");
    let artifact = request.output_directory.join(format!(
        "{}-{}.msi",
        binary_name(manifest),
        manifest.application.version
    ));
    let wix_source = request
        .output_directory
        .join(format!("{}.wxs", binary_name(manifest)));
    let mut steps = common_staging_steps(request, &staging, &binary, "resources");
    steps.push(BundleStep::WriteText {
        destination: wix_source.clone(),
        contents: wix_source_document(manifest, upgrade_code, &staging, &binary),
    });
    steps.push(run_tool(
        "wix",
        [
            "build".to_owned(),
            path_string(&wix_source),
            "-o".to_owned(),
            path_string(&artifact),
        ],
    ));
    append_windows_signing(manifest, &artifact, &mut steps);
    Ok(BundlePlan {
        format: request.format,
        staging_directory: staging,
        artifact_path: artifact,
        steps,
    })
}

fn plan_windows_nsis(
    manifest: &AppManifest,
    request: &BundleRequest,
) -> Result<BundlePlan, BundlePlanError> {
    let binary = windows_binary_name(manifest);
    let staging = request.output_directory.join("nsis");
    let artifact = request.output_directory.join(format!(
        "{}-{}-setup.exe",
        binary_name(manifest),
        manifest.application.version
    ));
    let script = request
        .output_directory
        .join(format!("{}.nsi", binary_name(manifest)));
    let mut steps = common_staging_steps(request, &staging, &binary, "resources");
    steps.push(BundleStep::WriteText {
        destination: script.clone(),
        contents: nsis_script(manifest, &staging, &artifact, &binary),
    });
    steps.push(run_tool("makensis", [path_string(&script)]));
    append_windows_signing(manifest, &artifact, &mut steps);
    Ok(BundlePlan {
        format: request.format,
        staging_directory: staging,
        artifact_path: artifact,
        steps,
    })
}

fn plan_macos_app(
    manifest: &AppManifest,
    request: &BundleRequest,
) -> Result<BundlePlan, BundlePlanError> {
    let application_directory = request
        .output_directory
        .join(format!("{}.app", manifest.application.name));
    let contents = application_directory.join("Contents");
    let macos = contents.join("MacOS");
    let resources = contents.join("Resources");
    let binary = binary_name(manifest);
    let mut steps = vec![
        BundleStep::ResetDirectory(application_directory.clone()),
        BundleStep::CreateDirectory(macos.clone()),
        BundleStep::CreateDirectory(resources.clone()),
        BundleStep::CopyFile {
            source: request.executable.clone(),
            destination: macos.join(&binary),
        },
    ];
    append_resources(request, &resources, &mut steps);
    if let Some(icon) = &manifest.bundle.icons.macos_icns {
        steps.push(BundleStep::CopyFile {
            source: icon.clone(),
            destination: resources.join("AppIcon.icns"),
        });
    }
    steps.push(BundleStep::WriteText {
        destination: contents.join("Info.plist"),
        contents: macos_info_plist(manifest, &binary),
    });
    append_macos_signing(manifest, &application_directory, &mut steps);
    Ok(BundlePlan {
        format: request.format,
        staging_directory: application_directory.clone(),
        artifact_path: application_directory,
        steps,
    })
}

fn plan_macos_dmg(
    manifest: &AppManifest,
    request: &BundleRequest,
) -> Result<BundlePlan, BundlePlanError> {
    let mut app_request = request.clone();
    app_request.format = PackageFormat::MacOsApp;
    let app_plan = plan_macos_app(manifest, &app_request)?;
    let artifact = request.output_directory.join(format!(
        "{}-{}.dmg",
        manifest.application.name, manifest.application.version
    ));
    let mut steps = app_plan.steps;
    steps.push(run_tool(
        "hdiutil",
        [
            "create".to_owned(),
            "-volname".to_owned(),
            manifest.application.name.clone(),
            "-srcfolder".to_owned(),
            path_string(&app_plan.artifact_path),
            "-ov".to_owned(),
            "-format".to_owned(),
            "UDZO".to_owned(),
            path_string(&artifact),
        ],
    ));
    if let Some(profile) = &manifest.bundle.macos.notarization_profile {
        steps.push(run_tool(
            "xcrun",
            [
                "notarytool".to_owned(),
                "submit".to_owned(),
                path_string(&artifact),
                "--keychain-profile".to_owned(),
                profile.clone(),
                "--wait".to_owned(),
            ],
        ));
    }
    Ok(BundlePlan {
        format: request.format,
        staging_directory: app_plan.staging_directory,
        artifact_path: artifact,
        steps,
    })
}

fn plan_linux_app_image(
    manifest: &AppManifest,
    request: &BundleRequest,
) -> Result<BundlePlan, BundlePlanError> {
    let binary = binary_name(manifest);
    let staging = request.output_directory.join(format!("{binary}.AppDir"));
    let desktop = desktop_file(manifest, &binary);
    let mut steps = linux_staging_steps(manifest, request, &staging, &binary, &desktop);
    let app_run = staging.join("AppRun");
    steps.push(BundleStep::WriteText {
        destination: app_run.clone(),
        contents: format!("#!/bin/sh\nexec \"$APPDIR/usr/bin/{binary}\" \"$@\"\n"),
    });
    steps.push(BundleStep::SetExecutable(app_run));
    let artifact = request.output_directory.join(format!(
        "{binary}-{}.AppImage",
        manifest.application.version
    ));
    steps.push(run_tool(
        "appimagetool",
        [path_string(&staging), path_string(&artifact)],
    ));
    Ok(BundlePlan {
        format: request.format,
        staging_directory: staging,
        artifact_path: artifact,
        steps,
    })
}

fn plan_linux_deb(
    manifest: &AppManifest,
    request: &BundleRequest,
) -> Result<BundlePlan, BundlePlanError> {
    let binary = binary_name(manifest);
    let staging = request.output_directory.join("deb-root");
    let desktop = desktop_file(manifest, &binary);
    let mut steps = linux_staging_steps(manifest, request, &staging, &binary, &desktop);
    steps.push(BundleStep::WriteText {
        destination: staging.join("DEBIAN/control"),
        contents: debian_control(manifest, &binary),
    });
    let artifact = request.output_directory.join(format!(
        "{binary}_{}_amd64.deb",
        manifest.application.version
    ));
    steps.push(run_tool(
        "dpkg-deb",
        [
            "--root-owner-group".to_owned(),
            "--build".to_owned(),
            path_string(&staging),
            path_string(&artifact),
        ],
    ));
    Ok(BundlePlan {
        format: request.format,
        staging_directory: staging,
        artifact_path: artifact,
        steps,
    })
}

fn plan_linux_rpm(
    manifest: &AppManifest,
    request: &BundleRequest,
) -> Result<BundlePlan, BundlePlanError> {
    let binary = binary_name(manifest);
    let staging = request.output_directory.join("rpm-root");
    let desktop = desktop_file(manifest, &binary);
    let mut steps = linux_staging_steps(manifest, request, &staging, &binary, &desktop);
    let top_directory = request.output_directory.join("rpmbuild");
    let specification = top_directory.join("SPECS").join(format!("{binary}.spec"));
    steps.push(BundleStep::CreateDirectory(
        top_directory.join("RPMS/x86_64"),
    ));
    steps.push(BundleStep::CreateDirectory(top_directory.join("BUILD")));
    steps.push(BundleStep::WriteText {
        destination: specification.clone(),
        contents: rpm_specification(manifest, &binary, &staging),
    });
    steps.push(run_tool(
        "rpmbuild",
        [
            "-bb".to_owned(),
            "--define".to_owned(),
            format!("_topdir {}", path_string(&top_directory)),
            path_string(&specification),
        ],
    ));
    let artifact = top_directory.join("RPMS/x86_64").join(format!(
        "{binary}-{}-1.x86_64.rpm",
        manifest.application.version
    ));
    Ok(BundlePlan {
        format: request.format,
        staging_directory: staging,
        artifact_path: artifact,
        steps,
    })
}

fn common_staging_steps(
    request: &BundleRequest,
    staging: &Path,
    binary: &str,
    resource_directory: &str,
) -> Vec<BundleStep> {
    let mut steps = vec![
        BundleStep::ResetDirectory(staging.to_owned()),
        BundleStep::CopyFile {
            source: request.executable.clone(),
            destination: staging.join(binary),
        },
    ];
    append_resources(request, &staging.join(resource_directory), &mut steps);
    steps
}

fn linux_staging_steps(
    manifest: &AppManifest,
    request: &BundleRequest,
    staging: &Path,
    binary: &str,
    desktop: &str,
) -> Vec<BundleStep> {
    let executable_directory = staging.join("usr/bin");
    let share_directory = staging.join("usr/share");
    let mut steps = vec![
        BundleStep::ResetDirectory(staging.to_owned()),
        BundleStep::CreateDirectory(executable_directory.clone()),
        BundleStep::CopyFile {
            source: request.executable.clone(),
            destination: executable_directory.join(binary),
        },
        BundleStep::WriteText {
            destination: share_directory
                .join("applications")
                .join(format!("{}.desktop", manifest.application.id)),
            contents: desktop.to_owned(),
        },
    ];
    append_resources(
        request,
        &share_directory.join(binary).join("resources"),
        &mut steps,
    );
    if let Some(icon_directory) = &manifest.bundle.icons.linux_png_directory {
        steps.push(BundleStep::CopyFile {
            source: icon_directory.join("256x256.png"),
            destination: share_directory
                .join("icons/hicolor/256x256/apps")
                .join(format!("{}.png", manifest.application.id)),
        });
    }
    steps
}

fn append_resources(request: &BundleRequest, root: &Path, steps: &mut Vec<BundleStep>) {
    for resource in &request.bundled_resources {
        steps.push(BundleStep::CopyFile {
            source: resource.source.clone(),
            destination: root.join(
                resource
                    .logical_path
                    .replace('/', std::path::MAIN_SEPARATOR_STR),
            ),
        });
    }
}

fn append_windows_signing(manifest: &AppManifest, artifact: &Path, steps: &mut Vec<BundleStep>) {
    if let Some(thumbprint) = &manifest.bundle.windows.certificate_thumbprint {
        steps.push(run_tool(
            "signtool",
            [
                "sign".to_owned(),
                "/sha1".to_owned(),
                thumbprint.clone(),
                "/fd".to_owned(),
                "SHA256".to_owned(),
                path_string(artifact),
            ],
        ));
    }
}

fn append_macos_signing(
    manifest: &AppManifest,
    application_directory: &Path,
    steps: &mut Vec<BundleStep>,
) {
    if let Some(identity) = &manifest.bundle.macos.signing_identity {
        let mut arguments = vec![
            "--force".to_owned(),
            "--deep".to_owned(),
            "--sign".to_owned(),
            identity.clone(),
        ];
        if manifest.bundle.macos.hardened_runtime {
            arguments.extend(["--options".to_owned(), "runtime".to_owned()]);
        }
        if let Some(entitlements) = &manifest.bundle.macos.entitlements {
            arguments.extend(["--entitlements".to_owned(), path_string(entitlements)]);
        }
        arguments.push(path_string(application_directory));
        steps.push(run_tool("codesign", arguments));
    }
}

fn execute_step(step: &BundleStep) -> Result<(), BundleExecutionError> {
    match step {
        BundleStep::ResetDirectory(path) => reset_directory(path),
        BundleStep::CreateDirectory(path) => {
            fs::create_dir_all(path).map_err(|source| BundleExecutionError::Filesystem {
                path: path.clone(),
                source,
            })
        }
        BundleStep::CopyFile {
            source,
            destination,
        } => {
            create_parent(destination)?;
            fs::copy(source, destination)
                .map(|_bytes| ())
                .map_err(|source_error| BundleExecutionError::Filesystem {
                    path: destination.clone(),
                    source: source_error,
                })
        }
        BundleStep::WriteText {
            destination,
            contents,
        } => {
            create_parent(destination)?;
            fs::write(destination, contents).map_err(|source| BundleExecutionError::Filesystem {
                path: destination.clone(),
                source,
            })
        }
        BundleStep::SetExecutable(path) => set_executable(path),
        BundleStep::RunTool {
            program,
            arguments,
            current_directory,
        } => {
            let mut command = Command::new(program);
            command.args(arguments);
            if let Some(current_directory) = current_directory {
                command.current_dir(current_directory);
            }
            let status = command
                .status()
                .map_err(|source| BundleExecutionError::ToolStart {
                    program: program.clone(),
                    source,
                })?;
            if status.success() {
                Ok(())
            } else {
                Err(BundleExecutionError::ToolFailed {
                    program: program.clone(),
                    status: status.to_string(),
                })
            }
        }
    }
}

fn reset_directory(path: &Path) -> Result<(), BundleExecutionError> {
    if path.parent().is_none() || path.components().count() < 2 {
        return Err(BundleExecutionError::UnsafeReset(path.to_owned()));
    }
    if path.exists() {
        fs::remove_dir_all(path).map_err(|source| BundleExecutionError::Filesystem {
            path: path.to_owned(),
            source,
        })?;
    }
    fs::create_dir_all(path).map_err(|source| BundleExecutionError::Filesystem {
        path: path.to_owned(),
        source,
    })
}

fn create_parent(path: &Path) -> Result<(), BundleExecutionError> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    fs::create_dir_all(parent).map_err(|source| BundleExecutionError::Filesystem {
        path: parent.to_owned(),
        source,
    })
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), BundleExecutionError> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)
        .map_err(|source| BundleExecutionError::Filesystem {
            path: path.to_owned(),
            source,
        })?
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).map_err(|source| BundleExecutionError::Filesystem {
        path: path.to_owned(),
        source,
    })
}

#[cfg(not(unix))]
fn set_executable(path: &Path) -> Result<(), BundleExecutionError> {
    if path.is_file() {
        Ok(())
    } else {
        Err(BundleExecutionError::Filesystem {
            path: path.to_owned(),
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "generated executable does not exist",
            ),
        })
    }
}

fn run_tool(program: impl Into<String>, arguments: impl IntoIterator<Item = String>) -> BundleStep {
    BundleStep::RunTool {
        program: program.into(),
        arguments: arguments.into_iter().collect(),
        current_directory: None,
    }
}

fn binary_name(manifest: &AppManifest) -> String {
    manifest
        .application
        .binary_name
        .as_deref()
        .unwrap_or(manifest.application.name.as_str())
        .to_owned()
}

fn windows_binary_name(manifest: &AppManifest) -> String {
    format!("{}.exe", binary_name(manifest))
}

fn windows_package_version(
    manifest: &AppManifest,
    format: PackageFormat,
) -> Result<String, BundlePlanError> {
    let version = &manifest.application.version;
    if version.major > u16::MAX.into()
        || version.minor > u16::MAX.into()
        || version.patch > u16::MAX.into()
    {
        return Err(BundlePlanError::UnsupportedVersion {
            format,
            version: version.to_string(),
        });
    }
    Ok(format!(
        "{}.{}.{}.0",
        version.major, version.minor, version.patch
    ))
}

fn application_metadata_json(manifest: &AppManifest, format: PackageFormat) -> String {
    format!(
        "{{\n  \"id\": \"{}\",\n  \"name\": {},\n  \"version\": \"{}\",\n  \"format\": \"{:?}\"\n}}\n",
        manifest.application.id,
        json_string(&manifest.application.name),
        manifest.application.version,
        format
    )
}

fn windows_appx_manifest(
    manifest: &AppManifest,
    publisher: &str,
    version: &str,
    binary: &str,
) -> String {
    let application = &manifest.application;
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10" xmlns:uap="http://schemas.microsoft.com/appx/manifest/uap/windows10" IgnorableNamespaces="uap">
  <Identity Name="{id}" Publisher="{publisher}" Version="{version}" ProcessorArchitecture="neutral" />
  <Properties>
    <DisplayName>{name}</DisplayName>
    <PublisherDisplayName>{publisher_name}</PublisherDisplayName>
    <Logo>Assets\StoreLogo.png</Logo>
  </Properties>
  <Applications>
    <Application Id="App" Executable="{binary}" EntryPoint="Windows.FullTrustApplication">
      <uap:VisualElements DisplayName="{name}" Description="{name}" BackgroundColor="transparent" Square150x150Logo="Assets\Square150x150Logo.png" Square44x44Logo="Assets\Square44x44Logo.png" />
    </Application>
  </Applications>
</Package>
"#,
        id = xml_escape(application.id.as_str()),
        publisher = xml_escape(publisher),
        version = version,
        name = xml_escape(&application.name),
        publisher_name = xml_escape(&application.publisher),
        binary = xml_escape(binary),
    )
}

fn wix_source_document(
    manifest: &AppManifest,
    upgrade_code: &str,
    staging: &Path,
    binary: &str,
) -> String {
    format!(
        r#"<Wix xmlns="http://wixtoolset.org/schemas/v4/wxs">
  <Package Name="{name}" Manufacturer="{publisher}" Version="{version}" UpgradeCode="{upgrade_code}">
    <MajorUpgrade DowngradeErrorMessage="A newer version of {name} is already installed." />
    <MediaTemplate EmbedCab="yes" />
    <StandardDirectory Id="ProgramFiles6432Folder">
      <Directory Id="INSTALLFOLDER" Name="{name}">
        <Component Id="MainExecutable" Guid="*">
          <File Source="{source}" Name="{binary}" KeyPath="yes" />
        </Component>
      </Directory>
    </StandardDirectory>
    <Feature Id="ProductFeature"><ComponentRef Id="MainExecutable" /></Feature>
  </Package>
</Wix>
"#,
        name = xml_escape(&manifest.application.name),
        publisher = xml_escape(&manifest.application.publisher),
        version = manifest.application.version,
        upgrade_code = xml_escape(upgrade_code),
        source = xml_escape(&path_string(&staging.join(binary))),
        binary = xml_escape(binary),
    )
}

fn nsis_script(manifest: &AppManifest, staging: &Path, artifact: &Path, binary: &str) -> String {
    format!(
        "Unicode true\nName {}\nOutFile {}\nInstallDir \"$PROGRAMFILES64\\{}\"\nRequestExecutionLevel user\nSection\n  SetOutPath \"$INSTDIR\"\n  File /r \"{}\\*\"\n  CreateShortcut \"$DESKTOP\\{}.lnk\" \"$INSTDIR\\{}\"\nSectionEnd\n",
        nsis_quote(&manifest.application.name),
        nsis_quote(&path_string(artifact)),
        manifest.application.name.replace('"', ""),
        path_string(staging).replace('/', "\\"),
        manifest.application.name.replace('"', ""),
        binary,
    )
}

fn macos_info_plist(manifest: &AppManifest, binary: &str) -> String {
    let minimum = manifest
        .bundle
        .macos
        .minimum_system_version
        .as_deref()
        .map_or(String::new(), |version| {
            format!(
                "  <key>LSMinimumSystemVersion</key><string>{}</string>\n",
                xml_escape(version)
            )
        });
    let icon = manifest
        .bundle
        .icons
        .macos_icns
        .as_ref()
        .map_or(String::new(), |_icon| {
            "  <key>CFBundleIconFile</key><string>AppIcon</string>\n".to_owned()
        });
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>{id}</string>
  <key>CFBundleName</key><string>{name}</string>
  <key>CFBundleDisplayName</key><string>{name}</string>
  <key>CFBundleExecutable</key><string>{binary}</string>
  <key>CFBundleShortVersionString</key><string>{version}</string>
  <key>CFBundleVersion</key><string>{version}</string>
  <key>NSHighResolutionCapable</key><true/>
{minimum}{icon}</dict></plist>
"#,
        id = xml_escape(manifest.application.id.as_str()),
        name = xml_escape(&manifest.application.name),
        binary = xml_escape(binary),
        version = manifest.application.version,
    )
}

fn desktop_file(manifest: &AppManifest, binary: &str) -> String {
    let categories = if manifest.bundle.linux.categories.is_empty() {
        "Utility;".to_owned()
    } else {
        format!("{};", manifest.bundle.linux.categories.join(";"))
    };
    let mime_types = if manifest.bundle.linux.mime_types.is_empty() {
        String::new()
    } else {
        format!("MimeType={};\n", manifest.bundle.linux.mime_types.join(";"))
    };
    format!(
        "[Desktop Entry]\nType=Application\nName={}\nExec={} %U\nIcon={}\nCategories={}\n{}Terminal=false\n",
        manifest.application.name, binary, manifest.application.id, categories, mime_types
    )
}

fn debian_control(manifest: &AppManifest, binary: &str) -> String {
    format!(
        "Package: {}\nVersion: {}\nSection: utils\nPriority: optional\nArchitecture: amd64\nMaintainer: {}\nDescription: {}\n",
        binary.to_ascii_lowercase(),
        manifest.application.version,
        manifest.application.publisher,
        manifest.application.name
    )
}

fn rpm_specification(manifest: &AppManifest, binary: &str, staging: &Path) -> String {
    format!(
        "Name: {binary}\nVersion: {version}\nRelease: 1\nSummary: {name}\nLicense: Unspecified\nBuildArch: x86_64\n\n%description\n{name}\n\n%install\nmkdir -p %{{buildroot}}\ncp -a \"{staging}/.\" %{{buildroot}}/\n\n%files\n/usr/bin/{binary}\n/usr/share/**\n",
        version = manifest.application.version,
        name = manifest.application.name,
        staging = path_string(staging),
    )
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_error| "\"\"".to_owned())
}

fn nsis_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('$', "$$").replace('"', "$\\\""))
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use egpui_manifest::{
        AppManifest, ApplicationId, ApplicationMetadata, BundleManifest, LocaleId, PackageFormat,
        ResourceManifest, RuntimeManifest,
    };

    use super::{
        BundleRequest, BundlerBackend, HostPlatform, NativeBundlerBackend, formats_for_host,
    };

    fn manifest(format: PackageFormat) -> AppManifest {
        AppManifest {
            schema_version: 1,
            application: ApplicationMetadata {
                id: ApplicationId::new("com.example.app").expect("id"),
                name: "Example".to_owned(),
                version: "1.2.3".parse().expect("version"),
                publisher: "Example Publisher".to_owned(),
                copyright: None,
                default_locale: LocaleId::new("en-US").expect("locale"),
                binary_name: Some("example".to_owned()),
            },
            runtime: RuntimeManifest::default(),
            resources: ResourceManifest::default(),
            i18n: egpui_manifest::I18nManifest {
                source_locale: LocaleId::new("en-US").expect("locale"),
                locales: vec![LocaleId::new("en-US").expect("locale")],
                catalog_pattern: "locales/{locale}/main.ftl".to_owned(),
            },
            windows: BTreeMap::new(),
            bundle: BundleManifest {
                targets: vec![format],
                ..BundleManifest::default()
            },
        }
    }

    #[test]
    fn backend_reports_only_native_formats() {
        assert_eq!(
            NativeBundlerBackend.supported_formats(),
            formats_for_host(HostPlatform::current())
        );
    }

    #[test]
    fn plans_native_portable_directory() {
        if HostPlatform::current() != HostPlatform::Windows {
            return;
        }
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("egpui-bundle-{unique}"));
        fs::create_dir_all(&root).expect("root");
        let executable = root.join("example.exe");
        fs::write(&executable, b"test").expect("executable");
        let request = BundleRequest {
            executable,
            output_directory: root.join("output"),
            format: PackageFormat::WindowsPortable,
            bundled_resources: Vec::new(),
        };
        let plan = NativeBundlerBackend
            .plan(&manifest(request.format), &request)
            .expect("plan");
        assert!(plan.artifact_path.ends_with("example-portable"));
        fs::remove_dir_all(root).expect("cleanup");
    }
}
