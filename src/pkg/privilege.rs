use anyhow::{anyhow, Context, Result};
use std::path::Path;
use std::process::Command;
use which::which;

#[derive(Debug, Clone)]
pub struct PrivilegeManager {
    use_sudo: bool,
}

impl PrivilegeManager {
    pub fn new() -> Self {
        let use_sudo = Self::check_sudo_available();
        Self { use_sudo }
    }

    pub fn new_for_operation(requires_root: bool) -> Result<Self> {
        let manager = Self::new();

        if requires_root && !manager.is_root() {
            if !manager.use_sudo {
                return Err(anyhow!(
                    "Root privileges required but sudo is not available. \
                     Please run as root or install sudo."
                ));
            }
            if !manager.test_sudo() {
                return Err(anyhow!(
                    "sudo is available but authentication failed. \
                     Please ensure you have sudo access."
                ));
            }
        }

        Ok(manager)
    }

    fn check_sudo_available() -> bool {
        which("sudo").is_ok()
    }

    pub fn is_root(&self) -> bool {
        unsafe { libc::geteuid() == 0 }
    }

    pub fn test_sudo(&self) -> bool {
        if self.is_root() {
            return true;
        }

        Command::new("sudo")
            .args(["-n", "true"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    pub fn exec_as_root(&self, program: &str, args: &[&str]) -> Result<Command> {
        let mut cmd = if self.is_root() {
            Command::new(program)
        } else {
            Command::new("sudo")
        };

        if !self.is_root() {
            cmd.arg(program);
        }
        cmd.args(args);

        Ok(cmd)
    }

    pub fn run_pacman(&self, args: &[&str]) -> Result<Command> {
        self.exec_as_root("pacman", args)
    }

    pub fn run_with_privilege<F>(
        &self,
        mut cmd: Command,
        requires_root: bool,
    ) -> Result<std::process::Output>
    where
        F: FnOnce(&mut Command),
    {
        if requires_root && !self.is_root() {
            let program = cmd.get_program().to_string_lossy().to_string();
            let args: Vec<String> = cmd
                .get_args()
                .map(|s| s.to_string_lossy().to_string())
                .collect();

            let mut sudo_cmd = Command::new("sudo");
            sudo_cmd.arg(&program);
            sudo_cmd.args(&args);

            let output = sudo_cmd
                .output()
                .context("Failed to execute sudo command")?;
            return Ok(output);
        }

        cmd.output()
            .map_err(|e| anyhow!("Failed to execute command: {}", e))
    }
}

impl Default for PrivilegeManager {
    fn default() -> Self {
        Self::new()
    }
}

pub fn check_tool_available(tool: &str) -> Result<String> {
    let path = which(tool)
        .map(|p| p.to_string_lossy().to_string())
        .map_err(|_| anyhow!("Required tool '{}' not found in PATH", tool))?;
    Ok(path)
}

pub fn check_required_tools() -> Result<Vec<String>> {
    let mut available = Vec::new();

    let tools = ["pacman", "git", "bubblewrap"];

    for tool in tools {
        if let Ok(path) = check_tool_available(tool) {
            available.push(format!("{}: {}", tool, path));
        }
    }

    if available.is_empty() {
        return Err(anyhow!("None of the required tools are available"));
    }

    Ok(available)
}

pub fn check_arch_linux() -> Result<()> {
    let os_release = Path::new("/etc/os-release");

    if !os_release.exists() {
        return Err(anyhow!("Cannot verify OS: /etc/os-release not found"));
    }

    let content = std::fs::read_to_string(os_release)?;

    let is_arch = content
        .lines()
        .any(|line| line.starts_with("ID=arch") || line.starts_with("ID_LIKE=arch"));

    if !is_arch {
        return Err(anyhow!(
            "rscm only supports Arch Linux and derivatives. \
             Found /etc/os-release but no Arch Linux ID."
        ));
    }

    Ok(())
}

pub fn verify_build_environment() -> Result<()> {
    check_arch_linux()?;

    let tools = check_required_tools()?;

    if !tools.iter().any(|t| t.starts_with("pacman:")) {
        return Err(anyhow!("pacman is required but not found"));
    }

    Ok(())
}
