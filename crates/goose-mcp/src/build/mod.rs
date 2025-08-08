use anyhow::Result;
use chrono::{DateTime, Utc};
use indoc::indoc;
use mcp_core::handler::ToolError;
use mcp_core::protocol::ServerCapabilities;
use mcp_server::router::CapabilitiesBuilder;
use mcp_server::Router;
#[cfg(unix)]
use nix::{
    sys::signal::{kill as unix_kill, Signal},
    unistd::Pid,
};
use rmcp::model::{Content, JsonRpcMessage, Prompt, Resource, Role, Tool, ToolAnnotations};
use rmcp::object;
use serde_json::Value;
use std::fs::{self, File};
use std::future::Future;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use walkdir::WalkDir;
use zip::write::FileOptions;

use crate::developer::DeveloperRouter;

const PM: &str = "npm";

#[derive(Clone)]
pub struct BuildRouter {
    inner: DeveloperRouter,
    state: Arc<Mutex<BuildState>>, // shared process/log state
}

#[derive(Default)]
struct BuildState {
    child: Option<Child>,
    project_path: Option<PathBuf>,
    start_time: Option<DateTime<Utc>>,
    logs: Vec<String>,
    last_read_index: usize,
}

impl Default for BuildRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl BuildRouter {
    pub fn new() -> Self {
        let this = Self {
            inner: DeveloperRouter::new(),
            state: Arc::new(Mutex::new(BuildState::default())),
        };

        // Best-effort: run in background, ignore errors in constructor
        let state = this.state.clone();
        if let Ok(cwd) = std::env::current_dir() {
            let pkg_json = cwd.join("package.json");
            if pkg_json.exists() {
                tokio::spawn(async move {
                    {
                        let mut s = state.lock().unwrap();
                        s.project_path = Some(cwd.clone());
                    }
                    let _ = install_deps_if_needed(&cwd).await;
                    let _ = start_dev_server(state.clone(), &cwd).await;
                });
            }
        }

        this
    }
}

impl Router for BuildRouter {
    fn name(&self) -> String {
        "build".to_string()
    }

    fn instructions(&self) -> String {
        let mut base = self.inner.instructions();
        base.push_str(
            indoc! {
                "\n\nAdditional build tools available:\n- manage_server: {action: logs|restart|build|deploy, message?: string}\n"
            },
        );
        base
    }

    fn capabilities(&self) -> ServerCapabilities {
        CapabilitiesBuilder::new()
            .with_tools(false)
            .with_prompts(false)
            .build()
    }

    fn list_tools(&self) -> Vec<Tool> {
        let mut tools = self.inner.list_tools();

        let manage_server_tool = Tool::new(
            "manage_server",
            indoc! {
                r#"
                Manage the local web development project server and deployment lifecycle.

                Actions:
                - logs: Get logs emitted since the last time logs were checked (incremental)
                - restart: Install/update dependencies if needed and restart the dev server
                - build: Run the production build (npm run build)
                - deploy: Zip the build output and upload to the hosting endpoint

                Notes:
                - Autostarts the dev server on MCP startup when a package.json is present in the current directory
                - Uses npm for all operations
                - Deploy expects a .goose-metadata.json with { "subdomain": "..." } in the project root
                "#
            },
            object!({
                "type": "object",
                "required": ["action"],
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["logs", "restart", "build", "deploy"]
                    },
                    "message": {"type": "string", "description": "Optional deploy message"}
                }
            }),
        )
        .annotate(ToolAnnotations {
            title: Some("Manage development server".to_string()),
            read_only_hint: Some(false),
            destructive_hint: Some(false),
            idempotent_hint: Some(false),
            open_world_hint: Some(false),
        });

        tools.push(manage_server_tool);
        tools
    }

    fn call_tool(
        &self,
        tool_name: &str,
        arguments: Value,
        _notifier: mpsc::Sender<JsonRpcMessage>,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Vec<Content>, mcp_core::handler::ToolError>>
                + Send
                + 'static,
        >,
    > {
        let this = self.clone();
        let tool_name = tool_name.to_string();
        Box::pin(async move {
            match tool_name.as_str() {
                "manage_server" => this.manage_server(_notifier, arguments).await,
                _ => this.inner.call_tool(&tool_name, arguments, _notifier).await,
            }
        })
    }

    fn list_resources(&self) -> Vec<Resource> {
        self.inner.list_resources()
    }

    fn read_resource(
        &self,
        uri: &str,
    ) -> Pin<
        Box<dyn Future<Output = Result<String, mcp_core::handler::ResourceError>> + Send + 'static>,
    > {
        self.inner.read_resource(uri)
    }

    fn list_prompts(&self) -> Vec<Prompt> {
        self.inner.list_prompts()
    }

    fn get_prompt(
        &self,
        prompt_name: &str,
    ) -> Pin<
        Box<dyn Future<Output = Result<String, mcp_core::handler::PromptError>> + Send + 'static>,
    > {
        self.inner.get_prompt(prompt_name)
    }
}

impl BuildRouter {
    async fn manage_server(
        &self,
        _notifier: mpsc::Sender<JsonRpcMessage>,
        params: Value,
    ) -> Result<Vec<Content>, ToolError> {
        let action = params
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParameters("Missing 'action'".to_string()))?;

        // No package manager override; we always use npm

        match action {
            "logs" => {
                let (text, meta) = {
                    let mut s = self.state.lock().unwrap();
                    let start = s.last_read_index;
                    let end = s.logs.len();
                    let new_logs = s.logs[start..end].join("");
                    s.last_read_index = end;
                    let running = s.child.as_ref().is_some();
                    (
                        new_logs.to_string(),
                        format!("running: {running}, package_manager: {}", PM),
                    )
                };
                Ok(vec![
                    Content::text(meta).with_audience(vec![Role::Assistant]),
                    Content::text(text)
                        .with_audience(vec![Role::User])
                        .with_priority(0.0),
                ])
            }
            "restart" => {
                let (project, child_opt) = {
                    let mut s = self.state.lock().unwrap();
                    let project = s
                        .project_path
                        .clone()
                        .unwrap_or(std::env::current_dir().unwrap());

                    // take existing process if any (drop lock before await)
                    let child_opt = s.child.take();
                    s.logs.push(format!(
                        "[{}] Restart requested for project: {}\n",
                        Utc::now(),
                        project.display()
                    ));
                    (project, child_opt)
                };
                // kill outside lock
                if let Some(mut child) = child_opt {
                    // Try graceful shutdown first
                    let pid_opt = child.id().map(|id| id as i32);
                    {
                        let mut s = self.state.lock().unwrap();
                        if let Some(pid) = pid_opt {
                            s.logs.push(format!(
                                "[{}] Sending SIGTERM to pid {} for graceful shutdown...\n",
                                Utc::now(),
                                pid
                            ));
                        } else {
                            s.logs.push(format!(
                                "[{}] Child has no pid; attempting hard kill...\n",
                                Utc::now()
                            ));
                        }
                    }

                    #[cfg(unix)]
                    {
                        use tokio::time::{timeout, Duration};
                        if let Some(pid) = pid_opt {
                            let _ = unix_kill(Pid::from_raw(pid), Signal::SIGTERM);
                            match timeout(Duration::from_secs(3), child.wait()).await {
                                Ok(_status) => {
                                    let mut s = self.state.lock().unwrap();
                                    s.logs.push(format!(
                                        "[{}] Process {} exited cleanly after SIGTERM.\n",
                                        Utc::now(),
                                        pid
                                    ));
                                }
                                Err(_elapsed) => {
                                    {
                                        let mut s = self.state.lock().unwrap();
                                        s.logs.push(format!(
                                            "[{}] Timeout waiting for pid {} to exit, sending SIGKILL...\n",
                                            Utc::now(), pid
                                        ));
                                    }
                                    let _ = child.kill().await; // SIGKILL
                                    let _ = child.wait().await; // ensure it's fully terminated
                                }
                            }
                        } else {
                            let _ = child.kill().await;
                            let _ = child.wait().await;
                        }
                    }

                    #[cfg(not(unix))]
                    {
                        // On non-Unix, no SIGTERM; fall back to hard kill
                        let _ = child.kill().await;
                        let _ = child.wait().await;
                    }
                }

                // install deps if needed and start
                install_deps_if_needed(&project)
                    .await
                    .map_err(to_tool_err)?;
                start_dev_server(self.state.clone(), &project)
                    .await
                    .map_err(to_tool_err)?;

                Ok(vec![
                    Content::text("Server restarted").with_audience(vec![Role::Assistant])
                ])
            }
            "build" => {
                let project = {
                    let s = self.state.lock().unwrap();
                    s.project_path
                        .clone()
                        .unwrap_or(std::env::current_dir().unwrap())
                };
                let out = run_pm_command(&project, &pm_build_command()).await?;
                Ok(vec![Content::text(out).with_audience(vec![Role::User])])
            }
            "deploy" => {
                let message = params
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Deployment from Build MCP");
                let project = {
                    let s = self.state.lock().unwrap();
                    s.project_path
                        .clone()
                        .unwrap_or(std::env::current_dir().unwrap())
                };
                let report = deploy_from_project(&project, message).await?;
                Ok(vec![Content::text(report).with_audience(vec![Role::User])])
            }
            _ => Err(ToolError::InvalidParameters(format!(
                "Unsupported action: {}",
                action
            ))),
        }
    }
}

fn pm_install_command() -> Vec<&'static str> {
    vec!["npm", "install"]
}

fn pm_dev_command() -> Vec<&'static str> {
    vec!["npm", "run", "dev"]
}

fn pm_build_command() -> Vec<&'static str> {
    vec!["npm", "run", "build"]
}

async fn install_deps_if_needed(project_path: &Path) -> Result<()> {
    // Heuristic: if node_modules missing or older than package.json, run install
    let pkg_json = project_path.join("package.json");
    let node_modules = project_path.join("node_modules");
    let needs_install = if !node_modules.exists() {
        true
    } else {
        let pkg_mtime = fs::metadata(&pkg_json).and_then(|m| m.modified()).ok();
        let nm_mtime = fs::metadata(&node_modules).and_then(|m| m.modified()).ok();
        match (pkg_mtime, nm_mtime) {
            (Some(p), Some(n)) => p > n, // package.json newer than node_modules
            (Some(_), None) => true,
            _ => false,
        }
    };

    if needs_install {
        let _ = append_log(
            project_path,
            &format!("[{}] Installing dependencies with npm...\n", Utc::now()),
        );
        let out = run_pm_command(project_path, &pm_install_command()).await?;
        let _ = append_log(project_path, &format!("{out}\n"));
    }

    Ok(())
}

async fn start_dev_server(state: Arc<Mutex<BuildState>>, project_path: &Path) -> Result<()> {
    {
        let mut s = state.lock().unwrap();
        if s.child.is_some() {
            return Ok(()); // already running
        }
        s.logs.push(format!(
            "[{}] Starting dev server in {} using npm...\n",
            Utc::now(),
            project_path.display(),
        ));
    }

    let mut cmd = Command::new(pm_dev_command()[0]);
    for arg in &pm_dev_command()[1..] {
        cmd.arg(arg);
    }
    let mut child = cmd
        .current_dir(project_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    {
        let mut s = state.lock().unwrap();
        s.start_time = Some(Utc::now());
        s.child = Some(child);
    }

    // Spawn log readers
    if let Some(stdout) = stdout {
        let state_clone = state.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let mut s = state_clone.lock().unwrap();
                s.logs.push(format!("[stdout] {line}\n"));
            }
        });
    }
    if let Some(stderr) = stderr {
        let state_clone = state.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let mut s = state_clone.lock().unwrap();
                s.logs.push(format!("[stderr] {line}\n"));
            }
        });
    }

    Ok(())
}

async fn run_pm_command(project_path: &Path, cmd_vec: &[&str]) -> Result<String, ToolError> {
    let mut cmd = Command::new(cmd_vec[0]);
    for arg in &cmd_vec[1..] {
        cmd.arg(arg);
    }
    let output = cmd
        .current_dir(project_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| ToolError::ExecutionError(format!("Failed to run {:?}: {}", cmd_vec, e)))?;

    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    text.push_str(&String::from_utf8_lossy(&output.stderr));

    if !output.status.success() {
        return Err(ToolError::ExecutionError(format!(
            "Command {:?} failed with code {:?}:\n{}",
            cmd_vec,
            output.status.code(),
            text
        )));
    }

    Ok(text)
}

fn to_tool_err(e: anyhow::Error) -> ToolError {
    ToolError::ExecutionError(e.to_string())
}

fn append_log(project: &Path, _line: &str) -> Result<()> {
    let _ = project; // placeholder if we later want per-project files
                     // Currently just push to in-memory log; when called here we don't have access to state
    Ok(())
}

async fn deploy_from_project(project: &Path, message: &str) -> Result<String, ToolError> {
    // Read metadata
    let meta_path = project.join(".goose-metadata.json");
    if !meta_path.exists() {
        return Err(ToolError::ExecutionError(
            "No .goose-metadata.json found in project root".into(),
        ));
    }
    let mut meta_str = String::new();
    File::open(&meta_path)
        .map_err(|e| ToolError::ExecutionError(format!("Failed to open metadata: {}", e)))?
        .read_to_string(&mut meta_str)
        .map_err(|e| ToolError::ExecutionError(format!("Failed to read metadata: {}", e)))?;
    let meta: serde_json::Value = serde_json::from_str(&meta_str)
        .map_err(|e| ToolError::ExecutionError(format!("Invalid JSON in metadata: {}", e)))?;
    let subdomain = meta
        .get("subdomain")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::ExecutionError("'subdomain' missing in metadata".into()))?;

    // Ensure build exists
    let build_dir = project.join("build");
    if !build_dir.exists() {
        return Err(ToolError::ExecutionError(
            "Build directory not found. Run manage_server build first.".into(),
        ));
    }

    // Create archive in temp file
    let archive_path = project.join("archive.zip");
    if archive_path.exists() {
        let _ = fs::remove_file(&archive_path);
    }

    let file = File::create(&archive_path)
        .map_err(|e| ToolError::ExecutionError(format!("Failed to create archive: {}", e)))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    for entry in WalkDir::new(&build_dir).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_file() {
            let rel = path.strip_prefix(project).unwrap_or(path);
            zip.start_file(rel.to_string_lossy(), options)
                .map_err(|e| ToolError::ExecutionError(format!("Zip error: {}", e)))?;
            let mut f = File::open(path)
                .map_err(|e| ToolError::ExecutionError(format!("Zip read error: {}", e)))?;
            let mut buf = Vec::new();
            f.read_to_end(&mut buf)
                .map_err(|e| ToolError::ExecutionError(format!("Zip read error: {}", e)))?;
            zip.write_all(&buf)
                .map_err(|e| ToolError::ExecutionError(format!("Zip write error: {}", e)))?;
        }
    }
    zip.finish()
        .map_err(|e| ToolError::ExecutionError(format!("Zip finalize error: {}", e)))?;

    // Upload
    let api = "https://goose-dev-sites.stage.sqprod.co/api/v1";
    let url = format!("{}/sites/{}/upload", api, subdomain);

    let client = reqwest::Client::new();
    // Build multipart form with in-memory bytes to avoid path-based file helper differences
    let mut archive_bytes = Vec::new();
    File::open(&archive_path)
        .map_err(|e| ToolError::ExecutionError(format!("Failed to re-open archive: {}", e)))?
        .read_to_end(&mut archive_bytes)
        .map_err(|e| ToolError::ExecutionError(format!("Failed to read archive: {}", e)))?;

    let file_part = reqwest::multipart::Part::bytes(archive_bytes)
        .file_name("archive.zip".to_string())
        .mime_str("application/zip")
        .map_err(|e| ToolError::ExecutionError(format!("Failed to build multipart part: {}", e)))?;

    let form = reqwest::multipart::Form::new()
        .text("message", message.to_string())
        .part("file", file_part);

    let resp = client
        .post(url)
        .multipart(form)
        .send()
        .await
        .map_err(|e| ToolError::ExecutionError(format!("Upload failed: {}", e)))?;

    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| ToolError::ExecutionError(format!("Failed to read response: {}", e)))?;

    // Clean archive
    let _ = fs::remove_file(&archive_path);

    if !status.is_success() {
        return Err(ToolError::ExecutionError(format!(
            "Upload failed with status {}: {}",
            status, text
        )));
    }

    let site_url = format!("https://{}.vibeplatstage.squarecdn.com", subdomain);
    Ok(format!(
        "Successfully deployed to {}\nResponse: {}",
        site_url, text
    ))
}
