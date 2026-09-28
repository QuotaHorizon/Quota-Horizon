mod process;

use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use capacity_domain::{Diagnostic, DiagnosticSeverity, ReasonCode};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

const CHATGPT_BUNDLE_IDENTIFIER: &str = "com.openai.codex";
const CHATGPT_BUNDLED_CODEX: &str = "/Applications/ChatGPT.app/Contents/Resources/codex";
const BUNDLED_CODEX_PATHS: [&str; 2] = [
    "Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex",
    "Contents/Resources/codex",
];
const VERSION_OUTPUT_LIMIT: usize = 4 * 1024;
const REGISTRY_OUTPUT_LIMIT: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HostPlatform {
    Macos,
    Windows,
    Linux,
    Unknown,
}

impl HostPlatform {
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Macos
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else {
            Self::Unknown
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Macos => "macos",
            Self::Windows => "windows",
            Self::Linux => "linux",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone)]
pub struct DiscoveryOptions {
    pub platform: HostPlatform,
    pub architecture: String,
    pub explicit_path: Option<PathBuf>,
    pub path_environment: Option<OsString>,
    pub current_directory: PathBuf,
    pub macos_bundled_codex: PathBuf,
    pub version_timeout: Duration,
}

impl DiscoveryOptions {
    pub fn current(explicit_path: Option<PathBuf>) -> Self {
        Self {
            platform: HostPlatform::current(),
            architecture: env::consts::ARCH.to_owned(),
            explicit_path,
            path_environment: env::var_os("PATH"),
            current_directory: env::current_dir().unwrap_or_else(|_| std::env::temp_dir()),
            macos_bundled_codex: PathBuf::from(CHATGPT_BUNDLED_CODEX),
            version_timeout: Duration::from_secs(2),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CandidateSource {
    Explicit,
    MacosChatgptBundle,
    MacosBundleRegistry,
    Path,
}

impl CandidateSource {
    const fn is_preconfirmed(self) -> bool {
        matches!(self, Self::Explicit | Self::MacosChatgptBundle)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::MacosChatgptBundle => "macos_chatgpt_bundle",
            Self::MacosBundleRegistry => "macos_bundle_registry",
            Self::Path => "path",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileIdentityStrength {
    OsFileId,
    MetadataFingerprint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CandidateVerification {
    Verified,
    ConfirmationRequired,
    VersionTimeout,
    VersionFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CodexExecutableCandidate {
    pub executable_id: String,
    pub sources: Vec<CandidateSource>,
    pub discovered_paths: Vec<String>,
    pub canonical_path: String,
    pub file_identity: String,
    pub identity_strength: FileIdentityStrength,
    pub version: Option<String>,
    pub verification: CandidateVerification,
    pub requires_confirmation: bool,
    pub reason_codes: Vec<ReasonCode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryOutcome {
    Selected,
    ConfirmationRequired,
    Ambiguous,
    NotFound,
    VerificationFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DiscoveryReport {
    pub schema_version: String,
    pub platform: HostPlatform,
    pub architecture: String,
    pub outcome: DiscoveryOutcome,
    pub selected_executable_id: Option<String>,
    pub candidates: Vec<CodexExecutableCandidate>,
    pub diagnostics: Vec<Diagnostic>,
}

impl DiscoveryReport {
    pub fn selected_candidate(&self) -> Option<&CodexExecutableCandidate> {
        let selected_id = self.selected_executable_id.as_deref()?;
        self.candidates
            .iter()
            .find(|candidate| candidate.executable_id == selected_id)
    }

    pub const fn exit_code(&self) -> u8 {
        match self.outcome {
            DiscoveryOutcome::Selected => 0,
            DiscoveryOutcome::ConfirmationRequired
            | DiscoveryOutcome::Ambiguous
            | DiscoveryOutcome::NotFound
            | DiscoveryOutcome::VerificationFailed => 4,
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ProbeFailure {
    #[error("executable was not found")]
    NotFound,
    #[error("permission denied")]
    PermissionDenied,
    #[error("command timed out")]
    Timeout,
    #[error("command exited unsuccessfully")]
    NonZeroExit(Option<i32>),
    #[error("command output exceeded the configured limit")]
    OutputTooLarge,
    #[error("command returned invalid UTF-8")]
    InvalidUtf8,
    #[error("command returned an empty version")]
    EmptyOutput,
    #[error("command returned an unexpected version format")]
    UnexpectedFormat,
    #[error("I/O failure")]
    Io,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum ExecutableIdentityError {
    #[error("the Codex executable candidate is not verified")]
    CandidateNotVerified,
    #[error("the selected Codex executable changed after verification")]
    Changed,
}

pub trait DiscoveryProbe {
    fn probe_version(&self, executable: &Path, timeout: Duration) -> Result<String, ProbeFailure>;

    fn registered_macos_applications(
        &self,
        timeout: Duration,
    ) -> Result<Vec<PathBuf>, ProbeFailure>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemDiscoveryProbe;

impl DiscoveryProbe for SystemDiscoveryProbe {
    fn probe_version(&self, executable: &Path, timeout: Duration) -> Result<String, ProbeFailure> {
        let stdout = process::run_bounded(
            executable,
            &[OsStr::new("--version")],
            timeout,
            VERSION_OUTPUT_LIMIT,
        )?;
        let text = String::from_utf8(stdout).map_err(|_| ProbeFailure::InvalidUtf8)?;
        let version = text.lines().next().unwrap_or_default().trim();
        if version.is_empty() {
            return Err(ProbeFailure::EmptyOutput);
        }
        if version.len() > 256 || version.chars().any(char::is_control) {
            return Err(ProbeFailure::OutputTooLarge);
        }
        if !valid_codex_version(version) {
            return Err(ProbeFailure::UnexpectedFormat);
        }
        Ok(version.to_owned())
    }

    fn registered_macos_applications(
        &self,
        timeout: Duration,
    ) -> Result<Vec<PathBuf>, ProbeFailure> {
        if HostPlatform::current() != HostPlatform::Macos {
            return Ok(Vec::new());
        }
        let query = format!("kMDItemCFBundleIdentifier == '{CHATGPT_BUNDLE_IDENTIFIER}'");
        let stdout = process::run_bounded(
            Path::new("/usr/bin/mdfind"),
            &[OsStr::new(&query)],
            timeout,
            REGISTRY_OUTPUT_LIMIT,
        )?;
        let text = String::from_utf8(stdout).map_err(|_| ProbeFailure::InvalidUtf8)?;
        Ok(text
            .lines()
            .map(str::trim)
            .filter(|line| line.ends_with(".app"))
            .map(PathBuf::from)
            .collect())
    }
}

pub fn discover_current(explicit_path: Option<PathBuf>) -> DiscoveryReport {
    discover(
        &DiscoveryOptions::current(explicit_path),
        &SystemDiscoveryProbe,
    )
}

/// Prefer the current native reader over the legacy layout. Do not launch the
/// shell wrapper: provenance and replacement checks must bind the actual file.
pub fn bundled_codex_path(application: &Path) -> Option<PathBuf> {
    BUNDLED_CODEX_PATHS
        .iter()
        .map(|relative| application.join(relative))
        .find(|path| path.is_file())
}

/// Presentation only; executable trust still requires discovery verification.
pub fn is_bundled_codex_path(path: &Path) -> bool {
    path.ancestors().any(|ancestor| {
        ancestor.extension().is_some_and(|ext| ext == "app")
            && (BUNDLED_CODEX_PATHS
                .iter()
                .any(|relative| ancestor.join(relative) == path)
                || ancestor.join("Contents/Resources/codex-cli/bin/codex") == path)
    })
}

fn default_bundled_codex_path(legacy_path: &Path) -> Option<PathBuf> {
    // Keep the configurable legacy path used by callers/tests, while resolving
    // both supported layouts of the same installation.
    legacy_path
        .ancestors()
        .nth(3)
        .filter(|application| application.extension().is_some_and(|ext| ext == "app"))
        .and_then(bundled_codex_path)
        .or_else(|| legacy_path.is_file().then(|| legacy_path.to_owned()))
}

pub fn discover(options: &DiscoveryOptions, probe: &dyn DiscoveryProbe) -> DiscoveryReport {
    let mut diagnostics = Vec::new();
    let mut candidates = Vec::new();

    if let Some(explicit_path) = &options.explicit_path {
        collect_candidate(
            &mut candidates,
            &mut diagnostics,
            explicit_path,
            CandidateSource::Explicit,
        );
    } else {
        if options.platform == HostPlatform::Macos {
            if let Some(executable) = default_bundled_codex_path(&options.macos_bundled_codex) {
                collect_candidate(
                    &mut candidates,
                    &mut diagnostics,
                    &executable,
                    CandidateSource::MacosChatgptBundle,
                );
            }

            match probe.registered_macos_applications(options.version_timeout) {
                Ok(applications) => {
                    for application in applications {
                        if let Some(executable) = bundled_codex_path(&application) {
                            collect_candidate(
                                &mut candidates,
                                &mut diagnostics,
                                &executable,
                                CandidateSource::MacosBundleRegistry,
                            );
                        }
                    }
                }
                Err(_) => diagnostics.push(diagnostic(
                    "macos_registry_unavailable",
                    DiagnosticSeverity::Warning,
                    "The macOS application registry could not be queried.",
                )),
            }
        }

        collect_path_candidates(options, &mut candidates, &mut diagnostics);
    }

    for candidate in &mut candidates {
        candidate.requires_confirmation = !candidate
            .sources
            .iter()
            .copied()
            .any(CandidateSource::is_preconfirmed);
        if candidate.requires_confirmation {
            candidate.verification = CandidateVerification::ConfirmationRequired;
            candidate
                .reason_codes
                .push(reason("path_confirmation_required"));
            continue;
        }

        match probe.probe_version(
            Path::new(&candidate.canonical_path),
            options.version_timeout,
        ) {
            Ok(version) => {
                candidate.version = Some(version);
                candidate.verification = CandidateVerification::Verified;
            }
            Err(ProbeFailure::Timeout) => {
                candidate.verification = CandidateVerification::VersionTimeout;
                candidate.reason_codes.push(reason("version_timeout"));
            }
            Err(_) => {
                candidate.verification = CandidateVerification::VersionFailed;
                candidate.reason_codes.push(reason("version_failed"));
            }
        }
    }

    let (outcome, selected_executable_id) = select_candidate(&candidates, &mut diagnostics);
    DiscoveryReport {
        schema_version: "1.0".to_owned(),
        platform: options.platform,
        architecture: options.architecture.clone(),
        outcome,
        selected_executable_id,
        candidates,
        diagnostics,
    }
}

fn collect_path_candidates(
    options: &DiscoveryOptions,
    candidates: &mut Vec<CodexExecutableCandidate>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(path_environment) = &options.path_environment else {
        return;
    };
    let current_directory = fs::canonicalize(&options.current_directory).ok();
    let executable_name = if options.platform == HostPlatform::Windows {
        "codex.exe"
    } else {
        "codex"
    };

    for directory in env::split_paths(path_environment) {
        if directory.as_os_str().is_empty() {
            continue;
        }
        if current_directory.as_ref().is_some_and(|cwd| {
            fs::canonicalize(&directory)
                .ok()
                .is_some_and(|candidate| candidate == *cwd)
        }) {
            diagnostics.push(diagnostic(
                "cwd_path_entry_ignored",
                DiagnosticSeverity::Warning,
                "A PATH entry resolving to the current directory was ignored.",
            ));
            continue;
        }

        let executable = directory.join(executable_name);
        if executable.exists() {
            collect_candidate(candidates, diagnostics, &executable, CandidateSource::Path);
        }
    }
}

fn collect_candidate(
    candidates: &mut Vec<CodexExecutableCandidate>,
    diagnostics: &mut Vec<Diagnostic>,
    discovered_path: &Path,
    source: CandidateSource,
) {
    let validated = match validate_executable(discovered_path) {
        Ok(validated) => validated,
        Err(code) => {
            diagnostics.push(diagnostic(
                code,
                DiagnosticSeverity::Error,
                "A Codex executable candidate failed validation.",
            ));
            return;
        }
    };

    if let Some(existing) = candidates
        .iter_mut()
        .find(|candidate| candidate.file_identity == validated.file_identity)
    {
        if !existing.sources.contains(&source) {
            existing.sources.push(source);
        }
        let discovered = discovered_path.to_string_lossy().into_owned();
        if !existing.discovered_paths.contains(&discovered) {
            existing.discovered_paths.push(discovered);
        }
        return;
    }

    let executable_id = opaque_executable_id(&validated.file_identity);
    candidates.push(CodexExecutableCandidate {
        executable_id,
        sources: vec![source],
        discovered_paths: vec![discovered_path.to_string_lossy().into_owned()],
        canonical_path: validated.canonical_path.to_string_lossy().into_owned(),
        file_identity: validated.file_identity,
        identity_strength: validated.identity_strength,
        version: None,
        verification: CandidateVerification::VersionFailed,
        requires_confirmation: true,
        reason_codes: Vec::new(),
    });
}

struct ValidatedExecutable {
    canonical_path: PathBuf,
    file_identity: String,
    identity_strength: FileIdentityStrength,
}

fn validate_executable(path: &Path) -> Result<ValidatedExecutable, &'static str> {
    let canonical_path = fs::canonicalize(path).map_err(|_| "canonicalize_failed")?;
    let metadata = fs::metadata(&canonical_path).map_err(|_| "executable_metadata_failed")?;
    if !metadata.is_file() {
        return Err("executable_not_file");
    }
    if !is_executable(&canonical_path, &metadata) {
        return Err("executable_not_executable");
    }
    let (file_identity, identity_strength) = file_identity(&canonical_path, &metadata);
    Ok(ValidatedExecutable {
        canonical_path,
        file_identity,
        identity_strength,
    })
}

/// Re-check the selected file immediately before starting a new child.
///
/// This deliberately does not accept a replacement at the same path. A normal
/// Codex upgrade must pass through discovery and explicit selection again so
/// the new file identity and version are visible to the user.
pub fn verify_executable_identity(
    candidate: &CodexExecutableCandidate,
) -> Result<(), ExecutableIdentityError> {
    if candidate.verification != CandidateVerification::Verified {
        return Err(ExecutableIdentityError::CandidateNotVerified);
    }
    let validated = validate_executable(Path::new(&candidate.canonical_path))
        .map_err(|_| ExecutableIdentityError::Changed)?;
    if validated.canonical_path.to_string_lossy() != candidate.canonical_path
        || validated.file_identity != candidate.file_identity
        || validated.identity_strength != candidate.identity_strength
    {
        return Err(ExecutableIdentityError::Changed);
    }
    Ok(())
}

#[cfg(unix)]
fn is_executable(_path: &Path, metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(windows)]
fn is_executable(path: &Path, _metadata: &fs::Metadata) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
}

#[cfg(not(any(unix, windows)))]
fn is_executable(_path: &Path, metadata: &fs::Metadata) -> bool {
    metadata.is_file()
}

#[cfg(unix)]
fn file_identity(_path: &Path, metadata: &fs::Metadata) -> (String, FileIdentityStrength) {
    use std::os::unix::fs::MetadataExt;
    let (modified_seconds, modified_nanos) = modified_parts(metadata);
    (
        format!(
            "unix:{}:{}:{}:{}:{}",
            metadata.dev(),
            metadata.ino(),
            metadata.len(),
            modified_seconds,
            modified_nanos
        ),
        FileIdentityStrength::OsFileId,
    )
}

#[cfg(not(unix))]
fn file_identity(path: &Path, metadata: &fs::Metadata) -> (String, FileIdentityStrength) {
    let (modified_seconds, modified_nanos) = modified_parts(metadata);
    (
        format!(
            "metadata:{}:{}:{}:{}",
            path.to_string_lossy().to_lowercase(),
            metadata.len(),
            modified_seconds,
            modified_nanos
        ),
        FileIdentityStrength::MetadataFingerprint,
    )
}

fn modified_parts(metadata: &fs::Metadata) -> (u64, u32) {
    let modified = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .unwrap_or_default();
    (modified.as_secs(), modified.subsec_nanos())
}

fn select_candidate(
    candidates: &[CodexExecutableCandidate],
    diagnostics: &mut Vec<Diagnostic>,
) -> (DiscoveryOutcome, Option<String>) {
    if candidates.is_empty() {
        diagnostics.push(diagnostic(
            "codex_not_found",
            DiagnosticSeverity::Error,
            "No valid Codex executable candidate was found.",
        ));
        return (DiscoveryOutcome::NotFound, None);
    }
    if candidates.len() > 1 {
        diagnostics.push(diagnostic(
            "multiple_codex_executables",
            DiagnosticSeverity::Error,
            "Multiple different Codex executables were found; select one explicitly.",
        ));
        return (DiscoveryOutcome::Ambiguous, None);
    }

    let candidate = &candidates[0];
    match candidate.verification {
        CandidateVerification::Verified => (
            DiscoveryOutcome::Selected,
            Some(candidate.executable_id.clone()),
        ),
        CandidateVerification::ConfirmationRequired => {
            diagnostics.push(diagnostic(
                "path_confirmation_required",
                DiagnosticSeverity::Warning,
                "The PATH candidate must be confirmed with --codex before it is executed.",
            ));
            (DiscoveryOutcome::ConfirmationRequired, None)
        }
        CandidateVerification::VersionTimeout | CandidateVerification::VersionFailed => {
            diagnostics.push(diagnostic(
                "codex_version_unverified",
                DiagnosticSeverity::Error,
                "The Codex candidate could not be verified with a bounded --version call.",
            ));
            (DiscoveryOutcome::VerificationFailed, None)
        }
    }
}

fn opaque_executable_id(file_identity: &str) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in file_identity.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("codex-executable-{hash:016x}")
}

fn valid_codex_version(value: &str) -> bool {
    value.strip_prefix("codex-cli ").is_some_and(|version| {
        !version.is_empty()
            && version.len() <= 96
            && version.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+' | b'_')
            })
    })
}

fn diagnostic(
    code: &'static str,
    severity: DiagnosticSeverity,
    message: &'static str,
) -> Diagnostic {
    Diagnostic {
        code: reason(code),
        severity,
        message: message.to_owned(),
    }
}

fn reason(code: &'static str) -> ReasonCode {
    ReasonCode::new(code).expect("static reason code must be valid")
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use tempfile::TempDir;

    use super::*;

    #[derive(Default)]
    struct FakeProbe {
        versions: HashMap<PathBuf, Result<String, ProbeFailure>>,
        applications: Vec<PathBuf>,
    }

    impl DiscoveryProbe for FakeProbe {
        fn probe_version(
            &self,
            executable: &Path,
            _timeout: Duration,
        ) -> Result<String, ProbeFailure> {
            self.versions
                .get(executable)
                .cloned()
                .unwrap_or(Err(ProbeFailure::NotFound))
        }

        fn registered_macos_applications(
            &self,
            _timeout: Duration,
        ) -> Result<Vec<PathBuf>, ProbeFailure> {
            Ok(self.applications.clone())
        }
    }

    fn options(root: &TempDir, platform: HostPlatform) -> DiscoveryOptions {
        DiscoveryOptions {
            platform,
            architecture: "test-arch".to_owned(),
            explicit_path: None,
            path_environment: None,
            current_directory: root.path().to_owned(),
            macos_bundled_codex: root.path().join("ChatGPT.app/Contents/Resources/codex"),
            version_timeout: Duration::from_millis(50),
        }
    }

    fn make_executable(path: &Path) {
        fs::create_dir_all(path.parent().expect("test path has parent")).unwrap();
        fs::write(path, b"test executable").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    fn test_codex_path(parent: &Path) -> PathBuf {
        if cfg!(windows) {
            parent.join("codex.exe")
        } else {
            parent.join("codex")
        }
    }

    #[test]
    fn explicit_candidate_is_selected_after_version_verification() {
        let root = TempDir::new().unwrap();
        let executable = test_codex_path(root.path());
        make_executable(&executable);
        let canonical = fs::canonicalize(&executable).unwrap();
        let mut probe = FakeProbe::default();
        probe
            .versions
            .insert(canonical, Ok("codex-cli test".to_owned()));
        let mut options = options(&root, HostPlatform::Linux);
        options.explicit_path = Some(executable);

        let report = discover(&options, &probe);

        assert_eq!(report.outcome, DiscoveryOutcome::Selected);
        assert_eq!(
            report.candidates[0].sources,
            vec![CandidateSource::Explicit]
        );
        assert_eq!(
            report.candidates[0].version.as_deref(),
            Some("codex-cli test")
        );
    }

    #[test]
    fn verified_candidate_rejects_an_in_place_executable_replacement() {
        let root = TempDir::new().unwrap();
        let executable = test_codex_path(root.path());
        make_executable(&executable);
        let canonical = fs::canonicalize(&executable).unwrap();
        let mut probe = FakeProbe::default();
        probe
            .versions
            .insert(canonical, Ok("codex-cli test".to_owned()));
        let mut options = options(&root, HostPlatform::Linux);
        options.explicit_path = Some(executable.clone());
        let report = discover(&options, &probe);
        let candidate = report.selected_candidate().unwrap();

        assert_eq!(verify_executable_identity(candidate), Ok(()));
        fs::write(&executable, b"replacement executable with a new identity").unwrap();

        assert_eq!(
            verify_executable_identity(candidate),
            Err(ExecutableIdentityError::Changed)
        );
    }

    #[test]
    fn unverified_candidate_cannot_be_used_as_an_identity_guard() {
        let root = TempDir::new().unwrap();
        let bin = root.path().join("bin");
        let executable = test_codex_path(&bin);
        make_executable(&executable);
        let mut options = options(&root, HostPlatform::current());
        options.path_environment = Some(env::join_paths([&bin]).unwrap());
        let report = discover(&options, &FakeProbe::default());

        assert_eq!(
            verify_executable_identity(&report.candidates[0]),
            Err(ExecutableIdentityError::CandidateNotVerified)
        );
    }

    #[test]
    fn nonexistent_explicit_candidate_fails_closed() {
        let root = TempDir::new().unwrap();
        let mut options = options(&root, HostPlatform::Linux);
        options.explicit_path = Some(test_codex_path(&root.path().join("missing")));

        let report = discover(&options, &FakeProbe::default());

        assert_eq!(report.outcome, DiscoveryOutcome::NotFound);
        assert!(report.selected_executable_id.is_none());
    }

    #[test]
    fn path_candidate_requires_confirmation_and_is_not_probed() {
        let root = TempDir::new().unwrap();
        let bin = root.path().join("bin");
        let executable = test_codex_path(&bin);
        make_executable(&executable);
        let mut options = options(&root, HostPlatform::current());
        options.path_environment = Some(env::join_paths([&bin]).unwrap());

        let report = discover(&options, &FakeProbe::default());

        assert_eq!(report.outcome, DiscoveryOutcome::ConfirmationRequired);
        assert_eq!(
            report.candidates[0].verification,
            CandidateVerification::ConfirmationRequired
        );
        assert!(report.candidates[0].version.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn registry_candidate_requires_confirmation_and_is_not_probed() {
        let root = TempDir::new().unwrap();
        let application = root.path().join("Alternate ChatGPT.app");
        let executable = application.join("Contents/Resources/codex");
        make_executable(&executable);
        let mut probe = FakeProbe::default();
        probe.applications.push(application);
        let options = options(&root, HostPlatform::Macos);

        let report = discover(&options, &probe);

        assert_eq!(report.outcome, DiscoveryOutcome::ConfirmationRequired);
        assert_eq!(
            report.candidates[0].sources,
            vec![CandidateSource::MacosBundleRegistry]
        );
        assert!(report.candidates[0].version.is_none());
    }

    #[test]
    fn current_bundle_layout_is_selected_and_old_layout_remains_supported() {
        for relative in BUNDLED_CODEX_PATHS {
            let root = TempDir::new().unwrap();
            let executable = root.path().join("ChatGPT.app").join(relative);
            make_executable(&executable);
            let mut probe = FakeProbe::default();
            probe.versions.insert(
                fs::canonicalize(&executable).unwrap(),
                Ok("codex-cli bundled".into()),
            );
            let report = discover(&options(&root, HostPlatform::Macos), &probe);
            assert_eq!(report.outcome, DiscoveryOutcome::Selected);
            assert!(is_bundled_codex_path(Path::new(
                &report.selected_candidate().unwrap().canonical_path
            )));
        }
    }

    #[test]
    fn native_reader_wins_over_legacy_file_and_registry_finds_new_layout() {
        let root = TempDir::new().unwrap();
        let application = root.path().join("Relocated Codex.app");
        let current = application.join(BUNDLED_CODEX_PATHS[0]);
        make_executable(&current);
        make_executable(&application.join(BUNDLED_CODEX_PATHS[1]));
        assert_eq!(bundled_codex_path(&application), Some(current.clone()));
        let probe = FakeProbe {
            applications: vec![application],
            ..Default::default()
        };
        let report = discover(&options(&root, HostPlatform::Macos), &probe);
        assert_eq!(report.outcome, DiscoveryOutcome::ConfirmationRequired);
        assert_eq!(report.candidates.len(), 1);
        assert_eq!(
            report.candidates[0].canonical_path,
            fs::canonicalize(current).unwrap().to_string_lossy()
        );
        assert!(report.candidates[0].version.is_none());
    }

    #[test]
    fn bundle_lookup_rejects_missing_files_and_only_labels_supported_layouts() {
        let root = TempDir::new().unwrap();
        let application = root.path().join("Codex.app");
        fs::create_dir_all(application.join(BUNDLED_CODEX_PATHS[0])).unwrap();
        assert!(bundled_codex_path(&application).is_none());
        assert!(!is_bundled_codex_path(Path::new("/opt/homebrew/bin/codex")));
        assert!(!is_bundled_codex_path(Path::new(
            "/tmp/example.app/tools/codex"
        )));
        assert!(is_bundled_codex_path(
            &application.join("Contents/Resources/codex-cli/bin/codex")
        ));
    }

    #[cfg(unix)]
    #[test]
    fn multiple_file_identities_are_never_silently_selected() {
        let root = TempDir::new().unwrap();
        let bundle = root.path().join("ChatGPT.app/Contents/Resources/codex");
        let path_bin = root.path().join("bin");
        let path_codex = path_bin.join("codex");
        make_executable(&bundle);
        make_executable(&path_codex);
        let mut probe = FakeProbe::default();
        probe.versions.insert(
            fs::canonicalize(&bundle).unwrap(),
            Ok("codex-cli bundled".to_owned()),
        );
        let mut options = options(&root, HostPlatform::Macos);
        options.path_environment = Some(env::join_paths([&path_bin]).unwrap());

        let report = discover(&options, &probe);

        assert_eq!(report.outcome, DiscoveryOutcome::Ambiguous);
        assert_eq!(report.candidates.len(), 2);
        assert!(report.selected_executable_id.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_and_real_path_are_deduplicated_by_file_identity() {
        use std::os::unix::fs::symlink;

        let root = TempDir::new().unwrap();
        let bundle = root.path().join("ChatGPT.app/Contents/Resources/codex");
        let path_bin = root.path().join("bin");
        let linked = path_bin.join("codex");
        make_executable(&bundle);
        fs::create_dir_all(&path_bin).unwrap();
        symlink(&bundle, &linked).unwrap();
        let canonical = fs::canonicalize(&bundle).unwrap();
        let mut probe = FakeProbe::default();
        probe
            .versions
            .insert(canonical, Ok("codex-cli same".to_owned()));
        let mut options = options(&root, HostPlatform::Macos);
        options.path_environment = Some(env::join_paths([&path_bin]).unwrap());

        let report = discover(&options, &probe);

        assert_eq!(report.candidates.len(), 1);
        assert_eq!(report.outcome, DiscoveryOutcome::Selected);
        assert_eq!(report.candidates[0].sources.len(), 2);
    }

    #[test]
    fn current_directory_path_entry_is_ignored() {
        let root = TempDir::new().unwrap();
        make_executable(&test_codex_path(root.path()));
        let mut options = options(&root, HostPlatform::current());
        options.path_environment = Some(env::join_paths([root.path()]).unwrap());

        let report = discover(&options, &FakeProbe::default());

        assert_eq!(report.outcome, DiscoveryOutcome::NotFound);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|item| item.code.as_str() == "cwd_path_entry_ignored")
        );
    }

    #[test]
    fn version_timeout_prevents_selection() {
        let root = TempDir::new().unwrap();
        let executable = test_codex_path(root.path());
        make_executable(&executable);
        let canonical = fs::canonicalize(&executable).unwrap();
        let mut probe = FakeProbe::default();
        probe.versions.insert(canonical, Err(ProbeFailure::Timeout));
        let mut options = options(&root, HostPlatform::Linux);
        options.explicit_path = Some(executable);

        let report = discover(&options, &probe);

        assert_eq!(report.outcome, DiscoveryOutcome::VerificationFailed);
        assert!(report.selected_executable_id.is_none());
    }

    #[test]
    fn report_json_never_contains_probe_error_payloads() {
        let root = TempDir::new().unwrap();
        let report = discover(&options(&root, HostPlatform::Linux), &FakeProbe::default());
        let json = serde_json::to_string(&report).unwrap();

        assert!(json.contains("codex_not_found"));
        assert!(!json.contains("PATH="));
    }

    #[test]
    fn version_output_uses_a_strict_non_secret_format() {
        assert!(valid_codex_version("codex-cli 0.150.0-alpha.12.2"));
        assert!(valid_codex_version("codex-cli 1.0.0+preview_1"));
        assert!(!valid_codex_version("codex-cli "));
        assert!(!valid_codex_version("other-cli 1.0.0"));
        assert!(!valid_codex_version("codex-cli Bearer secret"));
    }
}
