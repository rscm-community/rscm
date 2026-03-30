use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub path: Option<String>,
    pub version: Option<String>,
    pub required: bool,
    pub status: ToolStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ToolStatus {
    Found,
    NotFound,
    VersionMismatch { expected: String, found: String },
}

impl fmt::Display for ToolStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ToolStatus::Found => write!(f, "✓ Found"),
            ToolStatus::NotFound => write!(f, "✗ Not found"),
            ToolStatus::VersionMismatch { expected, found } => {
                write!(
                    f,
                    "⚠ Version mismatch (expected {}, found {})",
                    expected, found
                )
            }
        }
    }
}

pub struct ToolchainManager {
    required_tools: Vec<Tool>,
    optional_tools: Vec<Tool>,
    system_info: SystemInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfo {
    pub os: String,
    pub os_version: String,
    pub architecture: String,
    pub kernel: String,
}

impl ToolchainManager {
    pub fn new() -> Self {
        Self {
            required_tools: vec![
                Tool {
                    name: "bash".to_string(),
                    path: None,
                    version: None,
                    required: true,
                    status: ToolStatus::NotFound,
                },
                Tool {
                    name: "coreutils".to_string(), // for ln, cp, etc.
                    path: None,
                    version: None,
                    required: true,
                    status: ToolStatus::NotFound,
                },
            ],
            optional_tools: vec![Tool {
                name: "git".to_string(),
                path: None,
                version: None,
                required: false,
                status: ToolStatus::NotFound,
            }],
            system_info: SystemInfo {
                os: String::new(),
                os_version: String::new(),
                architecture: String::new(),
                kernel: String::new(),
            },
        }
    }

    pub fn check_status(&mut self) -> Result<()> {
        self.collect_system_info()?;

        for tool in &mut self.required_tools {
            Self::check_tool(tool)?;
        }

        for tool in &mut self.optional_tools {
            Self::check_tool(tool)?;
        }

        Ok(())
    }

    fn check_tool(tool: &mut Tool) -> Result<()> {
        let path = match Self::find_tool(&tool.name) {
            Some(p) => p,
            None => {
                tool.status = ToolStatus::NotFound;
                return Ok(());
            }
        };

        tool.path = Some(path);

        tool.version = Self::get_tool_version(&tool.name)?;

        tool.status = ToolStatus::Found;
        Ok(())
    }

    fn find_tool(name: &str) -> Option<String> {
        if name == "coreutils" {
            let essential_bins = ["ln", "cp", "mv", "rm", "mkdir"];
            for bin in essential_bins {
                if which::which(bin).is_err() {
                    return None;
                }
            }
            return Some("/usr/bin/coreutils".to_string());
        }

        which::which(name)
            .ok()
            .map(|p| p.to_string_lossy().to_string())
    }

    fn get_tool_version(name: &str) -> Result<Option<String>> {
        let version = match name {
            "bash" => Self::get_bash_version()?,
            "git" => Self::get_git_version()?,
            _ => None,
        };

        Ok(version)
    }

    fn get_bash_version() -> Result<Option<String>> {
        let output = Command::new("bash")
            .arg("--version")
            .output()
            .context("Failed to execute bash --version")?;

        if output.status.success() {
            let version_str = String::from_utf8_lossy(&output.stdout);
            if let Some(line) = version_str.lines().next() {
                if let Some(ver_part) = line.split("version").nth(1) {
                    if let Some(ver) = ver_part.split_whitespace().next() {
                        return Ok(Some(ver.to_string()));
                    }
                }
            }
        }

        Ok(None)
    }

    fn get_git_version() -> Result<Option<String>> {
        let output = Command::new("git")
            .arg("--version")
            .output()
            .context("Failed to execute git --version")?;

        if output.status.success() {
            let version_str = String::from_utf8_lossy(&output.stdout);
            if let Some(ver) = version_str.split_whitespace().nth(2) {
                return Ok(Some(ver.to_string()));
            }
        }

        Ok(None)
    }

    fn collect_system_info(&mut self) -> Result<()> {
        if let Ok(content) = std::fs::read_to_string("/etc/os-release") {
            for line in content.lines() {
                if line.starts_with("NAME=") {
                    self.system_info.os = line
                        .trim_start_matches("NAME=")
                        .trim_matches('"')
                        .to_string();
                } else if line.starts_with("VERSION_ID=") {
                    self.system_info.os_version = line
                        .trim_start_matches("VERSION_ID=")
                        .trim_matches('"')
                        .to_string();
                }
            }
        }

        self.system_info.architecture = std::env::consts::ARCH.to_string();

        let output = Command::new("uname")
            .arg("-r")
            .output()
            .context("Failed to execute uname -r")?;

        if output.status.success() {
            self.system_info.kernel = String::from_utf8_lossy(&output.stdout).trim().to_string();
        }

        Ok(())
    }

    pub fn all_required_tools_available(&self) -> bool {
        self.required_tools
            .iter()
            .all(|t| t.status == ToolStatus::Found)
    }

    pub fn get_report(&self) -> ToolchainReport {
        ToolchainReport {
            system_info: self.system_info.clone(),
            required_tools: self.required_tools.clone(),
            optional_tools: self.optional_tools.clone(),
            ready: self.all_required_tools_available(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ToolchainReport {
    pub system_info: SystemInfo,
    pub required_tools: Vec<Tool>,
    pub optional_tools: Vec<Tool>,
    pub ready: bool,
}

impl fmt::Display for ToolchainReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "🔧 rscm Toolchain Status")?;
        writeln!(f, "=========================")?;
        writeln!(f)?;

        writeln!(f, "System Information:")?;
        writeln!(
            f,
            "  OS: {} {}",
            self.system_info.os, self.system_info.os_version
        )?;
        writeln!(f, "  Architecture: {}", self.system_info.architecture)?;
        writeln!(f, "  Kernel: {}", self.system_info.kernel)?;
        writeln!(f)?;

        writeln!(f, "Required Tools:")?;
        for tool in &self.required_tools {
            writeln!(f, "  {:<12} {}", tool.name, tool.status)?;
            if let Some(path) = &tool.path {
                writeln!(f, "    └─ {}", path)?;
            }
            if let Some(version) = &tool.version {
                writeln!(f, "    └─ v{}", version)?;
            }
        }
        writeln!(f)?;

        writeln!(f, "Optional Tools:")?;
        for tool in &self.optional_tools {
            writeln!(f, "  {:<12} {}", tool.name, tool.status)?;
            if let Some(path) = &tool.path {
                writeln!(f, "    └─ {}", path)?;
            }
            if let Some(version) = &tool.version {
                writeln!(f, "    └─ {}", version)?;
            }
        }
        writeln!(f)?;

        if self.ready {
            writeln!(f, "✅ Toolchain is ready")?;
        } else {
            writeln!(f, "❌ Toolchain is not ready - missing required tools")?;
            writeln!(f, "   Please install missing tools:")?;
            writeln!(f, "   - bash")?;
            writeln!(f, "   - coreutils (ln, cp, mv, rm, mkdir)")?;
        }

        Ok(())
    }
}
