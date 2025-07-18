pub mod custom_tool;
pub mod execute;
pub mod fs_read;
pub mod fs_write;
pub mod gh_issue;
pub mod knowledge;
pub mod thinking;
pub mod use_aws;

use std::collections::{
    HashMap,
    HashSet,
};
use std::io::Write;
use std::path::{
    Path,
    PathBuf,
};

use crossterm::queue;
use crossterm::style::{
    self,
    Color,
    Stylize,
};
use custom_tool::CustomTool;
use execute::ExecuteCommand;
use eyre::Result;
use fs_read::FsRead;
use fs_write::FsWrite;
use gh_issue::GhIssue;
use knowledge::Knowledge;
use serde::{
    Deserialize,
    Serialize,
};
use thinking::Thinking;
use use_aws::UseAws;

use super::consts::MAX_TOOL_RESPONSE_SIZE;
use super::util::images::RichImageBlocks;
use crate::database::Database;
use crate::database::settings::Setting;
use crate::os::Os;

/// Represents an executable tool use.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum Tool {
    FsRead(FsRead),
    FsWrite(FsWrite),
    ExecuteCommand(ExecuteCommand),
    UseAws(UseAws),
    Custom(CustomTool),
    GhIssue(GhIssue),
    Knowledge(Knowledge),
    Thinking(Thinking),
}

impl Tool {
    /// The display name of a tool
    pub fn display_name(&self) -> String {
        match self {
            Tool::FsRead(_) => "fs_read",
            Tool::FsWrite(_) => "fs_write",
            #[cfg(windows)]
            Tool::ExecuteCommand(_) => "execute_cmd",
            #[cfg(not(windows))]
            Tool::ExecuteCommand(_) => "execute_bash",
            Tool::UseAws(_) => "use_aws",
            Tool::Custom(custom_tool) => &custom_tool.name,
            Tool::GhIssue(_) => "gh_issue",
            Tool::Knowledge(_) => "knowledge",
            Tool::Thinking(_) => "thinking (prerelease)",
        }
        .to_owned()
    }

    /// Whether or not the tool should prompt the user to accept before [Self::invoke] is called.
    pub fn requires_acceptance(&self, _os: &Os) -> bool {
        match self {
            Tool::FsRead(_) => false,
            Tool::FsWrite(_) => true,
            Tool::ExecuteCommand(execute_command) => execute_command.requires_acceptance(),
            Tool::UseAws(use_aws) => use_aws.requires_acceptance(),
            Tool::Custom(_) => true,
            Tool::GhIssue(_) => false,
            Tool::Knowledge(_) => false,
            Tool::Thinking(_) => false,
        }
    }

    /// Invokes the tool asynchronously
    pub async fn invoke(&self, os: &Os, stdout: &mut impl Write) -> Result<InvokeOutput> {
        match self {
            Tool::FsRead(fs_read) => fs_read.invoke(os, stdout).await,
            Tool::FsWrite(fs_write) => fs_write.invoke(os, stdout).await,
            Tool::ExecuteCommand(execute_command) => execute_command.invoke(stdout).await,
            Tool::UseAws(use_aws) => use_aws.invoke(os, stdout).await,
            Tool::Custom(custom_tool) => custom_tool.invoke(os, stdout).await,
            Tool::GhIssue(gh_issue) => gh_issue.invoke(os, stdout).await,
            Tool::Knowledge(knowledge) => knowledge.invoke(os, stdout).await,
            Tool::Thinking(think) => think.invoke(stdout).await,
        }
    }

    /// Queues up a tool's intention in a human readable format
    pub async fn queue_description(&self, os: &Os, output: &mut impl Write) -> Result<()> {
        match self {
            Tool::FsRead(fs_read) => fs_read.queue_description(os, output).await,
            Tool::FsWrite(fs_write) => fs_write.queue_description(os, output),
            Tool::ExecuteCommand(execute_command) => execute_command.queue_description(output),
            Tool::UseAws(use_aws) => use_aws.queue_description(output),
            Tool::Custom(custom_tool) => custom_tool.queue_description(output),
            Tool::GhIssue(gh_issue) => gh_issue.queue_description(output),
            Tool::Knowledge(knowledge) => knowledge.queue_description(os, output).await,
            Tool::Thinking(thinking) => thinking.queue_description(output),
        }
    }

    /// Validates the tool with the arguments supplied
    pub async fn validate(&mut self, os: &Os) -> Result<()> {
        match self {
            Tool::FsRead(fs_read) => fs_read.validate(os).await,
            Tool::FsWrite(fs_write) => fs_write.validate(os).await,
            Tool::ExecuteCommand(execute_command) => execute_command.validate(os).await,
            Tool::UseAws(use_aws) => use_aws.validate(os).await,
            Tool::Custom(custom_tool) => custom_tool.validate(os).await,
            Tool::GhIssue(gh_issue) => gh_issue.validate(os).await,
            Tool::Knowledge(knowledge) => knowledge.validate(os).await,
            Tool::Thinking(think) => think.validate(os).await,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolPermission {
    pub trusted: bool,
}

#[derive(Debug, Clone)]
/// Holds overrides for tool permissions.
/// Tools that do not have an associated ToolPermission should use
/// their default logic to determine to permission.
pub struct ToolPermissions {
    // We need this field for any stragglers
    pub trust_all: bool,
    pub permissions: HashMap<String, ToolPermission>,
    // Store pending trust-tool patterns for MCP tools that may be loaded later
    pub pending_trusted_tools: HashSet<String>,
}

impl ToolPermissions {
    pub fn new(capacity: usize) -> Self {
        Self {
            trust_all: false,
            permissions: HashMap::with_capacity(capacity),
            pending_trusted_tools: HashSet::new(),
        }
    }

    pub fn is_trusted(&mut self, tool_name: &str) -> bool {
        // Check if we should trust from pending patterns first
        if self.should_trust_from_pending(tool_name) {
            self.trust_tool(tool_name);
            self.pending_trusted_tools.remove(tool_name);
        }

        self.trust_all || self.permissions.get(tool_name).is_some_and(|perm| perm.trusted)
    }

    /// Returns a label to describe the permission status for a given tool.
    pub fn display_label(&mut self, tool_name: &str) -> String {
        let is_trusted = self.is_trusted(tool_name);
        let has_setting = self.has(tool_name) || self.trust_all;

        match (has_setting, is_trusted) {
            (true, true) => format!("  {}", "trusted".dark_green().bold()),
            (true, false) => format!("  {}", "not trusted".dark_grey()),
            _ => self.default_permission_label(tool_name),
        }
    }

    pub fn trust_tool(&mut self, tool_name: &str) {
        self.permissions
            .insert(tool_name.to_string(), ToolPermission { trusted: true });
    }

    pub fn untrust_tool(&mut self, tool_name: &str) {
        self.trust_all = false;
        self.pending_trusted_tools.remove(tool_name);
        self.permissions
            .insert(tool_name.to_string(), ToolPermission { trusted: false });
    }

    pub fn reset(&mut self) {
        self.trust_all = false;
        self.permissions.clear();
        self.pending_trusted_tools.clear();
    }

    pub fn reset_tool(&mut self, tool_name: &str) {
        self.trust_all = false;
        self.permissions.remove(tool_name);
        self.pending_trusted_tools.remove(tool_name);
    }

    /// Add a pending trust pattern for tools that may be loaded later
    pub fn add_pending_trust_tool(&mut self, pattern: String) {
        self.pending_trusted_tools.insert(pattern);
    }

    /// Check if a tool should be trusted based on preceding trust declarations
    pub fn should_trust_from_pending(&self, tool_name: &str) -> bool {
        // Check for exact match
        self.pending_trusted_tools.contains(tool_name)
    }

    pub fn has(&mut self, tool_name: &str) -> bool {
        // Check if we should trust from pending tools first
        if self.should_trust_from_pending(tool_name) {
            self.trust_tool(tool_name);
            self.pending_trusted_tools.remove(tool_name);
        }

        self.permissions.contains_key(tool_name)
    }

    pub fn trust_from_database(&mut self, database: &Database) {
        database
            .settings
            .get_array::<String>(Setting::TrustedTools)
            .into_iter()
            .flatten()
            .for_each(|tool_name| self.trust_tool(&tool_name));
    }

    /// Provide default permission labels for the built-in set of tools.
    // This "static" way avoids needing to construct a tool instance.
    fn default_permission_label(&self, tool_name: &str) -> String {
        let label = match tool_name {
            "fs_read" => "trusted".dark_green().bold(),
            "fs_write" => "not trusted".dark_grey(),
            #[cfg(not(windows))]
            "execute_bash" => "trust read-only commands".dark_grey(),
            #[cfg(windows)]
            "execute_cmd" => "trust read-only commands".dark_grey(),
            "use_aws" => "trust read-only commands".dark_grey(),
            "report_issue" => "trusted".dark_green().bold(),
            "knowledge" => "trusted".dark_green().bold(),
            "thinking" => "trusted (prerelease)".dark_green().bold(),
            _ if self.trust_all => "trusted".dark_grey().bold(),
            _ => "not trusted".dark_grey(),
        };

        format!("{} {label}", "*".reset())
    }
}

/// A tool specification to be sent to the model as part of a conversation. Maps to
/// [BedrockToolSpecification].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    #[serde(alias = "inputSchema")]
    pub input_schema: InputSchema,
    #[serde(skip_serializing, default = "tool_origin")]
    pub tool_origin: ToolOrigin,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum ToolOrigin {
    Native,
    McpServer(String),
}

impl<'de> Deserialize<'de> for ToolOrigin {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        if s == "native___" {
            Ok(ToolOrigin::Native)
        } else {
            Ok(ToolOrigin::McpServer(s))
        }
    }
}

impl Serialize for ToolOrigin {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            ToolOrigin::Native => serializer.serialize_str("native___"),
            ToolOrigin::McpServer(server) => serializer.serialize_str(server),
        }
    }
}

impl std::fmt::Display for ToolOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolOrigin::Native => write!(f, "Built-in"),
            ToolOrigin::McpServer(server) => write!(f, "{} (MCP)", server),
        }
    }
}

fn tool_origin() -> ToolOrigin {
    ToolOrigin::Native
}

#[derive(Debug, Clone)]
pub struct QueuedTool {
    pub id: String,
    pub name: String,
    pub accepted: bool,
    pub tool: Tool,
}

/// The schema specification describing a tool's fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputSchema(pub serde_json::Value);

/// The output received from invoking a [Tool].
#[derive(Debug, Default)]
pub struct InvokeOutput {
    pub output: OutputKind,
}

impl InvokeOutput {
    pub fn as_str(&self) -> &str {
        match &self.output {
            OutputKind::Text(s) => s.as_str(),
            OutputKind::Json(j) => j.as_str().unwrap_or_default(),
            OutputKind::Images(_) => "",
        }
    }
}

#[non_exhaustive]
#[derive(Debug)]
pub enum OutputKind {
    Text(String),
    Json(serde_json::Value),
    Images(RichImageBlocks),
}

impl Default for OutputKind {
    fn default() -> Self {
        Self::Text(String::new())
    }
}

/// Performs tilde expansion and other required sanitization modifications for handling tool use
/// path arguments.
///
/// Required since path arguments are defined by the model.
#[allow(dead_code)]
pub fn sanitize_path_tool_arg(os: &Os, path: impl AsRef<Path>) -> PathBuf {
    let mut res = PathBuf::new();
    // Expand `~` only if it is the first part.
    let mut path = path.as_ref().components();
    match path.next() {
        Some(p) if p.as_os_str() == "~" => {
            res.push(os.env.home().unwrap_or_default());
        },
        Some(p) => res.push(p),
        None => return res,
    }
    for p in path {
        res.push(p);
    }
    // For testing scenarios, we need to make sure paths are appropriately handled in chroot test
    // file systems since they are passed directly from the model.
    os.fs.chroot_path(res)
}

/// Converts `path` to a relative path according to the current working directory `cwd`.
fn absolute_to_relative(cwd: impl AsRef<Path>, path: impl AsRef<Path>) -> Result<PathBuf> {
    let cwd = cwd.as_ref().canonicalize()?;
    let path = path.as_ref().canonicalize()?;
    let mut cwd_parts = cwd.components().peekable();
    let mut path_parts = path.components().peekable();

    // Skip common prefix
    while let (Some(a), Some(b)) = (cwd_parts.peek(), path_parts.peek()) {
        if a == b {
            cwd_parts.next();
            path_parts.next();
        } else {
            break;
        }
    }

    // ".." for any uncommon parts, then just append the rest of the path.
    let mut relative = PathBuf::new();
    for _ in cwd_parts {
        relative.push("..");
    }
    for part in path_parts {
        relative.push(part);
    }

    Ok(relative)
}

/// Small helper for formatting the path as a relative path, if able.
fn format_path(cwd: impl AsRef<Path>, path: impl AsRef<Path>) -> String {
    absolute_to_relative(cwd, path.as_ref())
        .map(|p| p.to_string_lossy().to_string())
        // If we have three consecutive ".." then it should probably just stay as an absolute path.
        .map(|p| {
            let three_up = format!("..{}..{}..", std::path::MAIN_SEPARATOR, std::path::MAIN_SEPARATOR);
            if p.starts_with(&three_up) {
                path.as_ref().to_string_lossy().to_string()
            } else {
                p
            }
        })
        .unwrap_or(path.as_ref().to_string_lossy().to_string())
}

fn supports_truecolor(os: &Os) -> bool {
    // Simple override to disable truecolor since shell_color doesn't use Context.
    !os.env.get("Q_DISABLE_TRUECOLOR").is_ok_and(|s| !s.is_empty())
        && shell_color::get_color_support().contains(shell_color::ColorSupport::TERM24BIT)
}

/// Helper function to display a purpose if available (for execute commands)
pub fn display_purpose(purpose: Option<&String>, updates: &mut impl Write) -> Result<()> {
    if let Some(purpose) = purpose {
        queue!(
            updates,
            style::Print(super::CONTINUATION_LINE),
            style::Print("\n"),
            style::Print(super::PURPOSE_ARROW),
            style::SetForegroundColor(Color::Blue),
            style::Print("Purpose: "),
            style::ResetColor,
            style::Print(purpose),
            style::Print("\n"),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::MAIN_SEPARATOR;

    use super::*;
    use crate::os::ACTIVE_USER_HOME;

    #[tokio::test]
    async fn test_tilde_path_expansion() {
        let os = Os::new().await.unwrap();

        let actual = sanitize_path_tool_arg(&os, "~");
        let expected_home = os.env.home().unwrap_or_default();
        assert_eq!(actual, os.fs.chroot_path(&expected_home), "tilde should expand");
        let actual = sanitize_path_tool_arg(&os, "~/hello");
        assert_eq!(
            actual,
            os.fs.chroot_path(expected_home.join("hello")),
            "tilde should expand"
        );
        let actual = sanitize_path_tool_arg(&os, "/~");
        assert_eq!(
            actual,
            os.fs.chroot_path("/~"),
            "tilde should not expand when not the first component"
        );
    }

    #[tokio::test]
    async fn test_format_path() {
        async fn assert_paths(cwd: &str, path: &str, expected: &str) {
            let os = Os::new().await.unwrap();
            let cwd = sanitize_path_tool_arg(&os, cwd);
            let path = sanitize_path_tool_arg(&os, path);
            let fs = os.fs;
            fs.create_dir_all(&cwd).await.unwrap();
            fs.create_dir_all(&path).await.unwrap();

            let formatted = format_path(&cwd, &path);

            if Path::new(expected).is_absolute() {
                // If the expected path is relative, we need to ensure it is relative to the cwd.
                let expected = fs.chroot_path_str(expected);

                assert!(formatted == expected, "Expected '{}' to be '{}'", formatted, expected);

                return;
            }

            assert!(
                formatted.contains(expected),
                "Expected '{}' to be '{}'",
                formatted,
                expected
            );
        }

        // Test relative path from src to Downloads (sibling directories)
        assert_paths(
            format!("{ACTIVE_USER_HOME}{MAIN_SEPARATOR}src").as_str(),
            format!("{ACTIVE_USER_HOME}{MAIN_SEPARATOR}Downloads").as_str(),
            format!("..{MAIN_SEPARATOR}Downloads").as_str(),
        )
        .await;

        // Test absolute path that should stay absolute (going up too many levels)
        assert_paths(
            format!("{ACTIVE_USER_HOME}{MAIN_SEPARATOR}projects{MAIN_SEPARATOR}some{MAIN_SEPARATOR}project").as_str(),
            format!("{ACTIVE_USER_HOME}{MAIN_SEPARATOR}other").as_str(),
            format!("{ACTIVE_USER_HOME}{MAIN_SEPARATOR}other").as_str(),
        )
        .await;
    }
}
